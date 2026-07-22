//! The Settings mic test: play a short calibration tone through the default
//! output device while recording the default microphone, then report
//! whether (and how strongly) the mic heard it. This is the same acoustic
//! bleed the echo canceller in `src-tauri/src/transcribe.rs` cleans up on
//! every real meeting — the mic test just makes it visible before a meeting,
//! rather than inferring it after one.
//!
//! [`run_mic_test`] drives the production capture path (`Capture::start`) for
//! a few seconds, not a bespoke simplified stream, so what it measures is
//! what a real capture would actually hear.

use std::time::{Duration, Instant};

use crate::capture::Capture;
use crate::convert;
use crate::error::AudioError;
use crate::frame::CaptureItem;
use crate::mix::downmix_to_mono_into;
use crate::playback;
use crate::resample::MonoResampler;
use crate::source::CaptureSource;

/// Sample rate the calibration tone is generated at and the mic capture is
/// analyzed at. 16 kHz comfortably covers the tone's 300 Hz-6 kHz sweep.
const ANALYSIS_SAMPLE_RATE: u32 = 16_000;

/// Tone length. Short enough that the test feels instant, long enough for
/// [`estimate_delay`] to have a stable signal to correlate against.
const CHIRP_DURATION_SECS: f32 = 1.0;

/// Silence recorded before the tone starts, used both to let the mic
/// stream's own start-up jitter settle and as the "how loud is nothing"
/// baseline [`classify`] compares the tone against.
const PRE_ROLL: Duration = Duration::from_millis(300);

/// Extra recording time after the tone ends, so a delayed echo (e.g. a
/// Bluetooth speaker's output latency) isn't cut off before it arrives.
const POST_ROLL: Duration = Duration::from_millis(700);

/// Item-channel capacity for the test's capture — matches the size
/// `src-tauri`'s real captures use; a few seconds of mono frames never gets
/// close to it.
const ITEM_CAPACITY: usize = 256;

/// Peak amplitude below which the whole recording is "nothing", regardless
/// of the tone: roughly -60 dBFS, well under any real room's noise floor.
const SILENCE_PEAK: f32 = 0.001;

/// Normalized cross-correlation score (0 = no relationship, 1 = a perfect
/// scaled copy) above which the mic is considered to have heard the tone.
const CORRELATION_THRESHOLD: f32 = 0.3;

/// [`run_mic_test`] failed outright (as opposed to succeeding with a
/// [`MicTestOutcome::MicSilent`] result, which is a normal, reportable
/// outcome, not an error).
#[derive(Debug, thiserror::Error)]
pub enum MicTestError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error("failed to play the test tone: {0}")]
    Playback(AudioError),
    #[error("the test tone's playback thread panicked")]
    PlaybackPanicked,
}

/// What the mic test found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MicTestOutcome {
    /// The mic didn't meaningfully hear the tone — channels separate
    /// cleanly, whether because of headphones or because the mic and
    /// speakers just don't couple in this room.
    Headphones,
    /// The mic clearly heard the tone played through the speakers: `echo_db`
    /// is how much louder it was than the pre-tone noise floor, `delay_ms`
    /// the acoustic delay between playing it and the mic picking it up.
    Speakers { echo_db: f32, delay_ms: f32 },
    /// The recording was silent throughout, tone included — too little
    /// signal to classify (muted, disconnected, or no permission).
    MicSilent,
}

/// Runs the mic test end to end: builds the tone, plays it while recording
/// the default microphone through the same [`Capture`] path a real meeting
/// uses, and classifies what came back.
pub fn run_mic_test() -> Result<MicTestOutcome, MicTestError> {
    let reference = chirp(ANALYSIS_SAMPLE_RATE, CHIRP_DURATION_SECS);

    let capture = Capture::start(CaptureSource::Microphone, ITEM_CAPACITY)?;
    let format = capture.format();
    let items = capture.items();
    let mut resampler = MonoResampler::new(format.sample_rate, ANALYSIS_SAMPLE_RATE)?;

    // Let the mic stream's start-up jitter settle before the tone starts —
    // frames captured during this sleep queue in the item channel rather
    // than being lost, so nothing is missed.
    std::thread::sleep(PRE_ROLL);

    let playback_reference = reference.clone();
    let playback_thread = std::thread::spawn(move || {
        playback::play_blocking(&playback_reference, ANALYSIS_SAMPLE_RATE)
    });

    let collect_until = Instant::now() + Duration::from_secs_f32(CHIRP_DURATION_SECS) + POST_ROLL;
    let mut captured = Vec::new();
    let mut mono_scratch = Vec::new();
    while Instant::now() < collect_until {
        if let Ok(CaptureItem::Frame(frame)) = items.recv_timeout(Duration::from_millis(100)) {
            mono_scratch.clear();
            downmix_to_mono_into(&frame.samples, frame.format.channels, &mut mono_scratch);
            captured.extend(resampler.push(&mono_scratch));
        }
    }
    captured.extend(resampler.flush());
    capture.stop();

    match playback_thread.join() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => return Err(MicTestError::Playback(err)),
        Err(_) => return Err(MicTestError::PlaybackPanicked),
    }

    let noise_floor_len = (PRE_ROLL.as_secs_f32() * ANALYSIS_SAMPLE_RATE as f32) as usize;
    let noise_floor = &captured[..noise_floor_len.min(captured.len())];
    Ok(classify(
        &reference,
        &captured,
        noise_floor,
        ANALYSIS_SAMPLE_RATE,
    ))
}

/// A logarithmic sine sweep from 300 Hz to 6 kHz at a fixed amplitude — log
/// rather than linear so the tone spends comparable time per octave. Pure
/// signal generation, no I/O, so it's cheap to cover with unit tests.
fn chirp(sample_rate: u32, duration_secs: f32) -> Vec<f32> {
    const START_HZ: f64 = 300.0;
    const END_HZ: f64 = 6_000.0;
    const AMPLITUDE: f32 = 0.3;

    let len = (sample_rate as f32 * duration_secs) as usize;
    let k = (END_HZ / START_HZ).ln() / duration_secs as f64;
    let mut phase = 0.0f64;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let t = i as f64 / sample_rate as f64;
        let instantaneous_hz = START_HZ * (k * t).exp();
        phase += 2.0 * std::f64::consts::PI * instantaneous_hz / sample_rate as f64;
        out.push(phase.sin() as f32 * AMPLITUDE);
    }
    out
}

/// Where (in samples) `reference` best matches somewhere inside `captured`,
/// and the normalized cross-correlation score at that lag. `None` if
/// `captured` is shorter than `reference` or `reference` is silent.
///
/// Runs a coarse search at an 8-sample stride first, then refines around the
/// best coarse lag at full resolution — full-resolution search over a
/// several-second buffer is a needless ~8x more multiply-adds for precision
/// this is never shown at (results are rounded to the millisecond).
fn estimate_delay(reference: &[f32], captured: &[f32]) -> Option<(usize, f32)> {
    if reference.is_empty() || captured.len() < reference.len() {
        return None;
    }
    let (_, reference_rms) = convert::levels(reference);
    if reference_rms <= 0.0 {
        return None;
    }
    let reference_energy: f32 = reference.iter().map(|&s| s * s).sum();
    let max_lag = captured.len() - reference.len();

    let score_at = |lag: usize| -> f32 {
        let window = &captured[lag..lag + reference.len()];
        let dot: f32 = reference.iter().zip(window).map(|(&r, &c)| r * c).sum();
        let window_energy: f32 = window.iter().map(|&s| s * s).sum();
        let denom = (reference_energy * window_energy).sqrt();
        if denom > 0.0 {
            dot / denom
        } else {
            0.0
        }
    };

    const COARSE_STRIDE: usize = 8;
    let mut best_lag = 0;
    let mut best_score = f32::MIN;
    for lag in (0..=max_lag).step_by(COARSE_STRIDE) {
        let score = score_at(lag);
        if score > best_score {
            best_score = score;
            best_lag = lag;
        }
    }
    let refine_start = best_lag.saturating_sub(COARSE_STRIDE);
    let refine_end = (best_lag + COARSE_STRIDE).min(max_lag);
    for lag in refine_start..=refine_end {
        let score = score_at(lag);
        if score > best_score {
            best_score = score;
            best_lag = lag;
        }
    }

    Some((best_lag, best_score))
}

/// Classifies a completed mic-test recording: [`MicTestOutcome::MicSilent`]
/// if nothing was captured at all, otherwise whether `captured` correlates
/// with `reference` strongly enough to call it [`MicTestOutcome::Speakers`],
/// falling back to [`MicTestOutcome::Headphones`].
///
/// `noise_floor` must be the prefix of `captured` recorded before playback
/// started (see `run_mic_test`'s `PRE_ROLL`) — both the "how loud is
/// nothing" baseline `echo_db` is measured against, and the zero point
/// `delay_ms` is measured from. Without subtracting it, `delay_ms` would
/// report the pre-roll length itself rather than the acoustic delay.
fn classify(
    reference: &[f32],
    captured: &[f32],
    noise_floor: &[f32],
    sample_rate: u32,
) -> MicTestOutcome {
    let (captured_peak, _) = convert::levels(captured);
    if captured_peak < SILENCE_PEAK {
        return MicTestOutcome::MicSilent;
    }

    match estimate_delay(reference, captured) {
        Some((lag, score)) if score >= CORRELATION_THRESHOLD => {
            let window_end = (lag + reference.len()).min(captured.len());
            let (_, echo_rms) = convert::levels(&captured[lag..window_end]);
            let (_, floor_rms) = convert::levels(noise_floor);
            let echo_db = 20.0 * (echo_rms / floor_rms.max(1e-6)).log10();
            let acoustic_lag = lag.saturating_sub(noise_floor.len());
            let delay_ms = acoustic_lag as f32 * 1000.0 / sample_rate as f32;
            MicTestOutcome::Speakers { echo_db, delay_ms }
        }
        _ => MicTestOutcome::Headphones,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cheap deterministic pseudo-random generator — avoids adding a
    /// `rand` dependency for synthetic "the mic heard something unrelated"
    /// test audio.
    fn lcg_noise(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state >> 16) as i16 as f32) / (4.0 * 32768.0)
            })
            .collect()
    }

    #[test]
    fn chirp_has_the_requested_length_and_stays_in_range() {
        let tone = chirp(ANALYSIS_SAMPLE_RATE, 1.0);
        assert_eq!(tone.len(), ANALYSIS_SAMPLE_RATE as usize);
        assert!(tone.iter().all(|&s| s.abs() <= 0.31));
        // A tone that never moves isn't a tone.
        assert!(tone.iter().any(|&s| s.abs() > 0.05));
    }

    #[test]
    fn estimate_delay_finds_a_known_offset() {
        let reference = chirp(ANALYSIS_SAMPLE_RATE, CHIRP_DURATION_SECS);
        let delay_samples = 1_200; // 75ms at 16kHz
        let mut captured = vec![0.0f32; delay_samples];
        captured.extend(reference.iter().map(|&s| s * 0.6));
        captured.extend(vec![0.0f32; 1_000]);

        let (lag, score) = estimate_delay(&reference, &captured).unwrap();
        assert_eq!(lag, delay_samples);
        assert!(score > 0.9, "expected a strong match, got {score}");
    }

    #[test]
    fn estimate_delay_rejects_a_too_short_capture() {
        let reference = chirp(ANALYSIS_SAMPLE_RATE, CHIRP_DURATION_SECS);
        let too_short = vec![0.0f32; reference.len() - 1];
        assert!(estimate_delay(&reference, &too_short).is_none());
    }

    #[test]
    fn classify_reports_mic_silent_for_a_near_empty_recording() {
        let reference = chirp(ANALYSIS_SAMPLE_RATE, CHIRP_DURATION_SECS);
        let captured = vec![0.0f32; ANALYSIS_SAMPLE_RATE as usize * 2];
        let noise_floor = &captured[..4_800];
        assert_eq!(
            classify(&reference, &captured, noise_floor, ANALYSIS_SAMPLE_RATE),
            MicTestOutcome::MicSilent
        );
    }

    #[test]
    fn classify_reports_headphones_for_unrelated_captured_audio() {
        let reference = chirp(ANALYSIS_SAMPLE_RATE, CHIRP_DURATION_SECS);
        // Loud enough to clear the silence floor, but uncorrelated with the
        // tone — the "mic heard the room, not the speakers" case.
        let captured = lcg_noise(ANALYSIS_SAMPLE_RATE as usize * 2, 99);
        let noise_floor = &captured[..4_800];
        assert_eq!(
            classify(&reference, &captured, noise_floor, ANALYSIS_SAMPLE_RATE),
            MicTestOutcome::Headphones
        );
    }

    #[test]
    fn classify_reports_speakers_for_a_delayed_attenuated_echo() {
        let reference = chirp(ANALYSIS_SAMPLE_RATE, CHIRP_DURATION_SECS);
        let noise_floor = vec![0.0f32; 4_800]; // 300ms of true silence (the pre-roll)
        let delay_samples = 1_360; // 85ms acoustic delay, beyond the pre-roll
        let mut captured = noise_floor.clone();
        captured.extend(vec![0.0f32; delay_samples]);
        captured.extend(reference.iter().map(|&s| s * 0.4));
        captured.extend(vec![0.0f32; 4_800]);

        let outcome = classify(&reference, &captured, &noise_floor, ANALYSIS_SAMPLE_RATE);
        match outcome {
            MicTestOutcome::Speakers { echo_db, delay_ms } => {
                assert!(
                    echo_db > 6.0,
                    "expected a clearly audible echo, got {echo_db} dB"
                );
                assert!(
                    (delay_ms - 85.0).abs() < 2.0,
                    "expected ~85ms delay, got {delay_ms}ms"
                );
            }
            other => panic!("expected Speakers, got {other:?}"),
        }
    }
}
