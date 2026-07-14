//! Manual recording tool: drives the real two-channel capture pipeline
//! (loopback + mic + combiner — the same path `DualCapture` gives the app)
//! and writes the result to WAV, for use as raw material for a benchmark
//! fixture (see `crates/kodama-transcribe/tests/data/benchmark/README.md`).
//!
//! An example rather than app code: it needs `hound`, a dev-dependency only
//! (see this crate's `Cargo.toml` — runtime persistence is a separate,
//! not-yet-built ticket), and driving it is a one-off human action during a
//! real meeting, not a shipping feature.
//!
//! Usage:
//!   cargo run -p kodama-audio --example record_meeting -- <output-prefix> [--max-secs N]
//!
//! Announce the recording and get participants' consent before running this
//! (FOUNDING_DOC §3.7). Recording starts immediately; press Enter to stop it
//! (or let `--max-secs` elapse). Writes `<prefix>_you_them.wav` (interleaved
//! stereo, L=mic/you, R=loopback/them), `<prefix>_you.wav`, and
//! `<prefix>_them.wav`.

use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hound::{SampleFormat, WavSpec, WavWriter};
use kodama_audio::{AlignedSession, DualCapture, DualStatus, SessionChannel};

const TWO_CHANNEL_SAMPLE_RATE: u32 = 48_000;
const FRAME_CAPACITY: usize = 256;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (out_prefix, max_secs) = parse_args(env::args().skip(1).collect())?;
    if let Some(parent) = out_prefix.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }

    eprintln!("Starting loopback + microphone capture...");
    let capture = DualCapture::new(FRAME_CAPACITY, TWO_CHANNEL_SAMPLE_RATE);
    let status = capture.start()?;
    if let Some(err) = &status.loopback.error {
        return Err(format!("loopback capture failed to start: {err}").into());
    }
    if let Some(err) = &status.microphone.error {
        return Err(format!("microphone capture failed to start: {err}").into());
    }
    eprintln!("Recording. Both sources are live.");

    match max_secs {
        Some(secs) => {
            eprintln!("Recording for {secs}s...");
            std::thread::sleep(Duration::from_secs(secs));
        }
        None => {
            eprintln!("Press Enter to stop recording.");
            let mut line = String::new();
            io::stdin().lock().read_line(&mut line)?;
        }
    }

    eprintln!("Stopping capture...");
    let stop_started = Instant::now();
    let (status, session) = capture.stop()?;
    let session = session.ok_or(
        "only one capture source attached — no two-channel session was produced; \
         check that both a default output device (loopback) and a default input \
         device (microphone) were available",
    )?;
    eprintln!("Stopped in {:?}.", stop_started.elapsed());

    print_summary(&status, &session);
    write_outputs(&out_prefix, &session)?;

    eprintln!(
        "\nWrote {p}_you_them.wav, {p}_you.wav, {p}_them.wav",
        p = out_prefix.display()
    );
    eprintln!(
        "Reminder: this is real recorded audio — keep participant consent and \
         retention posture in mind before committing any of it (FOUNDING_DOC §3.7)."
    );
    Ok(())
}

fn parse_args(args: Vec<String>) -> Result<(PathBuf, Option<u64>), Box<dyn std::error::Error>> {
    let mut prefix = None;
    let mut max_secs = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--max-secs" => {
                let value = iter.next().ok_or("--max-secs requires a value")?;
                max_secs = Some(value.parse::<u64>()?);
            }
            _ if prefix.is_none() => prefix = Some(PathBuf::from(arg)),
            other => return Err(format!("unexpected argument: {other}").into()),
        }
    }
    let prefix = prefix.ok_or("usage: record_meeting <output-prefix> [--max-secs N]")?;
    Ok((prefix, max_secs))
}

fn print_summary(status: &DualStatus, session: &AlignedSession) {
    let you = session.channel(SessionChannel::Mic);
    let them = session.channel(SessionChannel::System);
    let (you_peak, you_rms) = levels(you);
    let (them_peak, them_rms) = levels(them);

    eprintln!("\n--- Session summary ---");
    eprintln!("sample_rate: {} Hz", session.sample_rate());
    eprintln!("duration: {:.1}s", session.duration().as_secs_f64());
    eprintln!(
        "mic (you):       frames_captured={} frames_dropped={} peak={you_peak:.3} rms={you_rms:.3}",
        status.microphone.frames_captured, status.microphone.frames_dropped,
    );
    eprintln!(
        "loopback (them): frames_captured={} frames_dropped={} peak={them_peak:.3} rms={them_rms:.3}",
        status.loopback.frames_captured, status.loopback.frames_dropped,
    );
    if you_peak == 0.0 {
        eprintln!("WARNING: mic channel is pure silence — check the microphone.");
    }
    if them_peak == 0.0 {
        eprintln!("WARNING: system channel is pure silence — was anything playing?");
    }
}

/// Peak absolute amplitude and RMS of a buffer of f32 samples. Mirrors
/// `kodama_audio`'s internal `convert::levels` (not part of the crate's
/// public surface) for this tool's post-capture summary.
fn levels(samples: &[f32]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut peak = 0.0_f32;
    let mut sum_sq = 0.0_f32;
    for &s in samples {
        peak = peak.max(s.abs());
        sum_sq += s * s;
    }
    (peak, (sum_sq / samples.len() as f32).sqrt())
}

fn write_outputs(
    prefix: &Path,
    session: &AlignedSession,
) -> Result<(), Box<dyn std::error::Error>> {
    let sample_rate = session.sample_rate();
    write_wav(
        &with_suffix(prefix, "you_them"),
        2,
        sample_rate,
        &session.interleaved_stereo(),
    )?;
    write_wav(
        &with_suffix(prefix, "you"),
        1,
        sample_rate,
        session.channel(SessionChannel::Mic),
    )?;
    write_wav(
        &with_suffix(prefix, "them"),
        1,
        sample_rate,
        session.channel(SessionChannel::System),
    )?;
    Ok(())
}

/// Appends `_{suffix}.wav` to `prefix`'s filename, keeping its directory.
fn with_suffix(prefix: &Path, suffix: &str) -> PathBuf {
    let mut name = prefix
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(format!("_{suffix}.wav"));
    prefix.with_file_name(name)
}

fn write_wav(
    path: &Path,
    channels: u16,
    sample_rate: u32,
    samples: &[f32],
) -> Result<(), Box<dyn std::error::Error>> {
    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(path, spec)?;
    for &sample in samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(())
}
