//! Retained-recording writer: converts a finished session's two mono `f32`
//! channels into one 16-bit PCM stereo WAV — `L = mic/you, R = system/them`,
//! the channel convention the `record_meeting` example established. The WAV is
//! the *recording* artifact that pairs with a session's `.jsonl` transcript
//! (same filename stem, `docs/FILENAME_SCHEME.md`), so it keeps the capture
//! rate (48 kHz) rather than the engines' 16 kHz, and it is written from the
//! raw channels — no echo cancellation, no cleanup — because it exists to let
//! a distilled summary be checked against the source.
//!
//! Both entry points stage the file next to its final name and `rename` it
//! into place, so a crash mid-write never leaves a half-valid `.wav` where a
//! reader (retention, the note view) would find it: the caller chooses a
//! staging path whose location and extension are invisible to those readers.

use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::Path;

use hound::{SampleFormat, WavSpec, WavWriter};

use crate::spill::SpillReader;

/// cpal/dasp's f32-PCM convention (see `src-tauri`'s AEC framing and
/// [`crate::convert`]): full-scale is ±1.0, scaled against `i16::MIN`'s
/// magnitude so conversion never overflows on the clamp.
const PCM_I16_SCALE: f32 = 32768.0;

/// Samples pulled per channel per iteration on the spill-backed path — one
/// second at the 48 kHz capture rate, so peak residency stays a couple of
/// hundred KB regardless of session length.
const READ_CHUNK_SAMPLES: usize = 48_000;

/// Canonical RIFF/WAV header size, for the u32 size-field overflow guard.
const WAV_HEADER_BYTES: u64 = 44;

/// Stream both spilled channels into a stereo WAV at `out_path`, staging at
/// `staging_path` first. `sample_rate` is the rate the spills were written at
/// (the combiner's 48 kHz). Channel files of unequal length — a crash-truncated
/// tail, or one source outliving the other — are padded with silence so the
/// timeline stays aligned.
pub fn write_stereo_wav_from_spills(
    mic_path: &Path,
    system_path: &Path,
    sample_rate: u32,
    staging_path: &Path,
    out_path: &Path,
) -> io::Result<()> {
    // 4 bytes per f32 spill sample; a trailing partial sample is dropped by
    // the reader, and the integer division floors to match.
    let mic_samples = fs::metadata(mic_path)?.len() / 4;
    let system_samples = fs::metadata(system_path)?.len() / 4;
    ensure_wav_fits(mic_samples.max(system_samples), out_path)?;

    let mut mic = SpillReader::open(mic_path)?;
    let mut system = SpillReader::open(system_path)?;
    write_staged(staging_path, out_path, sample_rate, |writer| {
        loop {
            let mic_chunk = mic.next_chunk(READ_CHUNK_SAMPLES)?;
            let system_chunk = system.next_chunk(READ_CHUNK_SAMPLES)?;
            if mic_chunk.is_none() && system_chunk.is_none() {
                return Ok(());
            }
            // A chunk short-fills only at its file's EOF, so pairing the
            // readers chunk-for-chunk keeps them sample-aligned: once one
            // stream ends, its side is padded with silence to the other's end.
            write_frames(
                writer,
                mic_chunk.unwrap_or(&[]),
                system_chunk.unwrap_or(&[]),
            )?;
        }
    })
}

/// The in-memory fallback (no spill files were created): interleave the two
/// resident channels into a stereo WAV at `out_path`, staging at
/// `staging_path` first. The shorter channel is padded with silence.
pub fn write_stereo_wav_from_channels(
    mic: &[f32],
    system: &[f32],
    sample_rate: u32,
    staging_path: &Path,
    out_path: &Path,
) -> io::Result<()> {
    ensure_wav_fits(mic.len().max(system.len()) as u64, out_path)?;
    write_staged(staging_path, out_path, sample_rate, |writer| {
        write_frames(writer, mic, system)
    })
}

/// WAV stores its RIFF/data chunk sizes as u32, so a stream whose 16-bit
/// stereo PCM data plus header exceeds `u32::MAX` would silently wrap into a
/// corrupt file at finalize. Fail loudly instead — the transcript is the
/// primary artifact and survives regardless. (~4 GiB is ~6.2 h of 16-bit
/// stereo at 48 kHz.)
fn ensure_wav_fits(frames: u64, out_path: &Path) -> io::Result<()> {
    // 2 channels × 2 bytes per 16-bit sample.
    let data_bytes = frames * 4;
    if data_bytes + WAV_HEADER_BYTES > u64::from(u32::MAX) {
        return Err(io::Error::other(format!(
            "{}: session too long for a WAV file ({:.1} GiB of samples exceeds the ~4 GiB WAV size limit)",
            out_path.display(),
            data_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
        )));
    }
    Ok(())
}

/// Create the WAV at `staging_path`, fill it via `fill`, finalize, then rename
/// it over `out_path`. Any failure removes the staging file (best-effort) so a
/// broken write leaves nothing behind; a pre-existing `out_path` (a stale
/// leftover from a failed retention delete) is replaced — the fresh audio is
/// the correct pairing.
fn write_staged(
    staging_path: &Path,
    out_path: &Path,
    sample_rate: u32,
    fill: impl FnOnce(&mut WavWriter<BufWriter<File>>) -> io::Result<()>,
) -> io::Result<()> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let staged = (|| {
        let mut writer = WavWriter::create(staging_path, spec).map_err(io::Error::other)?;
        fill(&mut writer)?;
        writer.finalize().map_err(io::Error::other)
    })();
    let renamed = staged.and_then(|()| {
        // Windows `rename` refuses an existing destination; a missing one is
        // the normal case, not an error.
        match fs::remove_file(out_path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        fs::rename(staging_path, out_path)
    });
    if renamed.is_err() {
        let _ = fs::remove_file(staging_path);
    }
    renamed
}

/// Interleave one aligned pair of chunks into the writer, `L = mic`,
/// `R = system`, padding the shorter side with silence.
fn write_frames(
    writer: &mut WavWriter<BufWriter<File>>,
    mic: &[f32],
    system: &[f32],
) -> io::Result<()> {
    let frames = mic.len().max(system.len());
    for i in 0..frames {
        let left = mic.get(i).copied().unwrap_or(0.0);
        let right = system.get(i).copied().unwrap_or(0.0);
        writer
            .write_sample(f32_to_i16(left))
            .map_err(io::Error::other)?;
        writer
            .write_sample(f32_to_i16(right))
            .map_err(io::Error::other)?;
    }
    Ok(())
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample * PCM_I16_SCALE).clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn read_wav(path: &Path) -> (WavSpec, Vec<i16>) {
        let mut reader = hound::WavReader::open(path).unwrap();
        let spec = reader.spec();
        let samples = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        (spec, samples)
    }

    fn write_spill(path: &Path, samples: &[f32]) {
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for &sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn channels_write_interleaves_mic_left_system_right() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("session.wav");
        write_stereo_wav_from_channels(
            &[0.5, -0.5],
            &[0.25, -0.25],
            48_000,
            &dir.path().join(".audio.tmp"),
            &out,
        )
        .unwrap();

        let (spec, samples) = read_wav(&out);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, SampleFormat::Int);
        assert_eq!(samples, vec![16384, 8192, -16384, -8192]);
    }

    #[test]
    fn conversion_clamps_beyond_full_scale() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("session.wav");
        write_stereo_wav_from_channels(
            &[1.5, -1.5],
            &[1.0, -1.0],
            48_000,
            &dir.path().join(".audio.tmp"),
            &out,
        )
        .unwrap();

        let (_, samples) = read_wav(&out);
        // +1.0 scales to 32768 and clamps to i16::MAX; anything past full
        // scale clamps to the same rail instead of wrapping.
        assert_eq!(samples, vec![i16::MAX, i16::MAX, i16::MIN, i16::MIN]);
    }

    #[test]
    fn shorter_channel_is_padded_with_silence() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("session.wav");
        write_stereo_wav_from_channels(
            &[0.5, 0.5, 0.5],
            &[0.25],
            48_000,
            &dir.path().join(".audio.tmp"),
            &out,
        )
        .unwrap();

        let (_, samples) = read_wav(&out);
        assert_eq!(samples, vec![16384, 8192, 16384, 0, 16384, 0]);
    }

    #[test]
    fn spill_write_round_trips_and_pads_the_shorter_file() {
        let dir = tempdir().unwrap();
        let mic_path = dir.path().join("mic.f32le");
        let system_path = dir.path().join("system.f32le");
        write_spill(&mic_path, &[0.5, 0.5, 0.5]);
        write_spill(&system_path, &[-0.5]);

        let out = dir.path().join("session.wav");
        let staging = dir.path().join(".audio.tmp");
        write_stereo_wav_from_spills(&mic_path, &system_path, 48_000, &staging, &out).unwrap();

        let (spec, samples) = read_wav(&out);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(samples, vec![16384, -16384, 16384, 0, 16384, 0]);
        assert!(!staging.exists(), "staging file must not survive success");
    }

    #[test]
    fn rename_replaces_a_stale_existing_wav() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("session.wav");
        fs::write(&out, b"stale not-a-wav leftover").unwrap();

        write_stereo_wav_from_channels(
            &[0.5],
            &[0.5],
            48_000,
            &dir.path().join(".audio.tmp"),
            &out,
        )
        .unwrap();

        let (_, samples) = read_wav(&out);
        assert_eq!(samples, vec![16384, 16384]);
    }

    #[test]
    fn failed_fill_removes_the_staging_file() {
        let dir = tempdir().unwrap();
        let staging = dir.path().join(".audio.tmp");
        let out = dir.path().join("session.wav");
        let result = write_staged(&staging, &out, 48_000, |_| {
            Err(io::Error::other("injected failure"))
        });
        assert!(result.is_err());
        assert!(
            !staging.exists(),
            "staging file must be cleaned up on failure"
        );
        assert!(!out.exists(), "no output may appear on failure");
    }

    #[test]
    fn missing_spill_file_errors_before_staging() {
        let dir = tempdir().unwrap();
        let staging = dir.path().join(".audio.tmp");
        let out = dir.path().join("session.wav");
        let missing = dir.path().join("mic.f32le");
        let system_path = dir.path().join("system.f32le");
        write_spill(&system_path, &[0.1]);

        assert!(
            write_stereo_wav_from_spills(&missing, &system_path, 48_000, &staging, &out).is_err()
        );
        assert!(!staging.exists());
        assert!(!out.exists());
    }

    #[test]
    fn overflow_guard_rejects_a_session_past_the_riff_limit() {
        // 2 ch × 2 bytes: just past the u32 ceiling.
        let frames = (u64::from(u32::MAX) - WAV_HEADER_BYTES) / 4 + 1;
        assert!(ensure_wav_fits(frames, Path::new("session.wav")).is_err());
        assert!(ensure_wav_fits(frames - 1, Path::new("session.wav")).is_ok());
    }
}
