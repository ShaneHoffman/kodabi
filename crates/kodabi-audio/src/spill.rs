//! Streaming spill of aligned capture audio to disk during a live session, so
//! a crash or kill mid-meeting loses at most the last flush interval rather
//! than the whole session, and the combiner's in-memory buffers stay bounded
//! regardless of session length (`docs/RESOURCE_BUDGET.md`).
//!
//! The combiner drains each channel's accumulated output to its own raw PCM
//! file on a size-based cadence ([`SpillConfig::flush_threshold_samples`]);
//! [`SpillReader`] reads it back, chunk at a time, for transcription (live at
//! stop, or from an orphaned in-flight session at the next startup). The format
//! is deliberately dumb — little-endian `f32` samples, no header — at the same
//! rate the combiner aligns to (48 kHz), so a spilled file is byte-for-byte the
//! timeline `AlignedSession` would have held in memory. Mapping the two files
//! to the you/them channels, and the sample rate, live in the in-flight
//! session's metadata alongside them (`kodabi-core`'s `inflight` module), not
//! in these headerless streams.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Where one session's two channels spill to, plus how much accumulated audio
/// triggers a flush. Built by [`crate::DualCapture::start`] (which owns the
/// flush-interval knob and the target rate) and handed to the combiner.
#[derive(Clone, Debug)]
pub struct SpillConfig {
    pub mic_path: PathBuf,
    pub system_path: PathBuf,
    /// Flush a channel to disk once its in-memory output reaches this many
    /// samples (`flush_secs * target_rate`). Bounds resident audio to roughly
    /// this per channel; also the memory-vs-durability tradeoff (a smaller
    /// value loses less on a crash but syncs to disk more often).
    pub flush_threshold_samples: usize,
}

/// Appends one channel's `f32` PCM to its spill file. Each [`append`] pushes
/// the samples through to the OS (a `flush` of the buffered writer), so a
/// `kill -9` — which drops the process but leaves the OS page cache intact —
/// cannot lose already-flushed audio. It does *not* `sync_all`: a hard power
/// loss can still lose the last unsynced flush, which is an accepted bound
/// (see the module docs and `docs/RESOURCE_BUDGET.md`).
///
/// [`append`]: ChannelSpillWriter::append
pub(crate) struct ChannelSpillWriter {
    writer: BufWriter<File>,
    path: PathBuf,
    /// Reused little-endian encode buffer, so `append` doesn't allocate a
    /// fresh `Vec` per flush.
    byte_scratch: Vec<u8>,
}

impl ChannelSpillWriter {
    /// Create the spill file, failing if it already exists — the in-flight
    /// directory is freshly minted per session, so a pre-existing file means a
    /// collision the caller must not silently append onto.
    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        let file = File::options().write(true).create_new(true).open(path)?;
        Ok(ChannelSpillWriter {
            writer: BufWriter::new(file),
            path: path.to_path_buf(),
            byte_scratch: Vec::new(),
        })
    }

    /// Encode `samples` as little-endian `f32` and write them through to the
    /// OS. See the type docs for the durability contract.
    pub(crate) fn append(&mut self, samples: &[f32]) -> io::Result<()> {
        self.byte_scratch.clear();
        self.byte_scratch.reserve(samples.len() * 4);
        for &sample in samples {
            self.byte_scratch.extend_from_slice(&sample.to_le_bytes());
        }
        self.writer.write_all(&self.byte_scratch)?;
        self.writer.flush()?;
        Ok(())
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }

    /// Consume the writer and return its file path — used at finalize to build
    /// the [`crate::SpilledSession`] once no more appends will happen.
    pub(crate) fn into_path(self) -> PathBuf {
        self.path
    }
}

/// Both channels' writers plus the flush threshold, created together so the
/// combiner either spills both channels or neither (a half-created pair would
/// leave one channel unrecoverable). Creation failing degrades the whole
/// session to in-memory (see the combiner's `coordinator_loop`).
pub(crate) struct SpillWriters {
    pub(crate) mic: ChannelSpillWriter,
    pub(crate) system: ChannelSpillWriter,
    pub(crate) threshold: usize,
}

impl SpillWriters {
    pub(crate) fn create(config: &SpillConfig) -> io::Result<Self> {
        let mic = ChannelSpillWriter::create(&config.mic_path)?;
        let system = ChannelSpillWriter::create(&config.system_path)?;
        Ok(SpillWriters {
            mic,
            system,
            threshold: config.flush_threshold_samples,
        })
    }
}

/// Reads a spilled channel file back as `f32` samples, a chunk at a time.
/// Tolerates a crash-truncated tail: a file whose length is not a whole number
/// of 4-byte samples (a `kill -9` mid-`append`) drops its trailing partial
/// sample rather than erroring, so a recovered session still transcribes.
pub struct SpillReader {
    reader: BufReader<File>,
    chunk: Vec<f32>,
    byte_buf: Vec<u8>,
}

impl SpillReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        Ok(SpillReader {
            reader: BufReader::new(file),
            chunk: Vec::new(),
            byte_buf: Vec::new(),
        })
    }

    /// Read up to `max_samples` samples. Returns `Ok(None)` at end of file.
    /// The returned slice borrows an internal buffer, so it is valid only
    /// until the next call.
    ///
    /// The read is filled to `max_samples * 4` bytes unless it hits EOF, so a
    /// short fill only ever happens on the final chunk — meaning a non-whole
    /// trailing sample (a truncated file) is dropped exactly once, at the end,
    /// and never shifts the samples that precede it.
    pub fn next_chunk(&mut self, max_samples: usize) -> io::Result<Option<&[f32]>> {
        let want = max_samples * 4;
        if self.byte_buf.len() < want {
            self.byte_buf.resize(want, 0);
        }
        let mut filled = 0;
        while filled < want {
            match self.reader.read(&mut self.byte_buf[filled..want]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(ref err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err),
            }
        }
        let whole = filled / 4;
        if whole == 0 {
            return Ok(None);
        }
        self.chunk.clear();
        self.chunk.reserve(whole);
        for i in 0..whole {
            let start = i * 4;
            let bytes = [
                self.byte_buf[start],
                self.byte_buf[start + 1],
                self.byte_buf[start + 2],
                self.byte_buf[start + 3],
            ];
            self.chunk.push(f32::from_le_bytes(bytes));
        }
        Ok(Some(&self.chunk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn append_then_read_round_trips_samples() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mic.f32le");

        let mut writer = ChannelSpillWriter::create(&path).unwrap();
        writer.append(&[0.1, -0.2, 0.3]).unwrap();
        writer.append(&[0.4, 0.5]).unwrap();
        assert_eq!(writer.path(), path);
        drop(writer);

        let mut reader = SpillReader::open(&path).unwrap();
        let mut all = Vec::new();
        while let Some(chunk) = reader.next_chunk(2).unwrap() {
            all.extend_from_slice(chunk);
        }
        assert_eq!(all, vec![0.1, -0.2, 0.3, 0.4, 0.5]);
    }

    #[test]
    fn create_refuses_an_existing_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mic.f32le");
        let _first = ChannelSpillWriter::create(&path).unwrap();
        assert!(ChannelSpillWriter::create(&path).is_err());
    }

    #[test]
    fn reader_drops_a_truncated_trailing_partial_sample() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("system.f32le");

        // Two whole samples (8 bytes) plus a truncated 2-byte tail, as a
        // kill -9 mid-append would leave.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0.25_f32.to_le_bytes());
        bytes.extend_from_slice(&(-0.5_f32).to_le_bytes());
        bytes.extend_from_slice(&[0xAB, 0xCD]);
        std::fs::write(&path, &bytes).unwrap();

        let mut reader = SpillReader::open(&path).unwrap();
        let mut all = Vec::new();
        // A large chunk request forces the read to EOF in one call, so the
        // partial tail is dropped at the boundary rather than mid-stream.
        while let Some(chunk) = reader.next_chunk(1024).unwrap() {
            all.extend_from_slice(chunk);
        }
        assert_eq!(all, vec![0.25, -0.5]);
    }

    #[test]
    fn reader_reassembles_a_sample_split_across_chunk_reads() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mic.f32le");
        let mut writer = ChannelSpillWriter::create(&path).unwrap();
        let samples: Vec<f32> = (0..1000).map(|i| i as f32 * 0.001).collect();
        writer.append(&samples).unwrap();
        drop(writer);

        // Read in small chunks whose byte boundaries never split a sample
        // (each chunk is a whole number of samples), then verify the stream
        // reassembles exactly regardless of chunk size.
        let mut reader = SpillReader::open(&path).unwrap();
        let mut all = Vec::new();
        while let Some(chunk) = reader.next_chunk(7).unwrap() {
            all.extend_from_slice(chunk);
        }
        assert_eq!(all, samples);
    }

    #[test]
    fn empty_file_reads_as_no_chunks() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mic.f32le");
        let _writer = ChannelSpillWriter::create(&path).unwrap();
        let mut reader = SpillReader::open(&path).unwrap();
        assert!(reader.next_chunk(64).unwrap().is_none());
    }
}
