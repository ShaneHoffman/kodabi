//! Acoustic echo cancellation: a safe wrapper over a vendored speexdsp echo
//! canceller (see `vendor/README.md`), used to remove the speaker output a
//! microphone acoustically picks up (mic-to-speaker crosstalk) from a
//! recorded channel, given the played audio as a reference.
//!
//! [`EchoCanceller::cancel`] processes one fixed-size frame at a time: the
//! near end (what the mic heard) and the far end (what was played, the same
//! audio the loopback channel captured) in, the near end with echo removed
//! out. It has no knowledge of sessions, channels, or files — the streaming
//! adapter that pulls frames from a session's two channels lives in
//! `src-tauri/src/transcribe.rs` (`EchoCancelledSource`), which is the only
//! caller.

mod ffi;

use std::ffi::c_void;
use std::os::raw::c_int;
use std::ptr;

/// [`EchoCanceller::new`] or [`EchoCanceller::cancel`] failed.
#[derive(Debug, thiserror::Error)]
pub enum AecError {
    #[error("speexdsp echo canceller failed to initialize")]
    EchoInitFailed,
    #[error("speexdsp preprocessor failed to initialize")]
    PreprocessInitFailed,
    #[error("frame length {actual} does not match the configured frame size {expected}")]
    FrameSizeMismatch { expected: usize, actual: usize },
}

/// One echo canceller instance per channel pair being cleaned. Not `Sync`
/// (speexdsp state is not safe to touch from two threads at once) but `Send`
/// so it can be built on one thread and used on another.
pub struct EchoCanceller {
    echo: ptr::NonNull<ffi::SpeexEchoState>,
    preprocess: ptr::NonNull<ffi::SpeexPreprocessState>,
    frame_size: usize,
}

// SAFETY: `SpeexEchoState`/`SpeexPreprocessState` are only ever touched
// through `&mut self` methods on `EchoCanceller`, so ownership transfer
// across a thread boundary (not concurrent access) is the only requirement.
unsafe impl Send for EchoCanceller {}

impl EchoCanceller {
    /// Builds a canceller for `frame_size`-sample frames (10-20 ms is
    /// speexdsp's documented sweet spot) at `sample_rate` Hz, cancelling up
    /// to `filter_length` samples of echo tail (should cover the acoustic
    /// delay between a speaker and the microphone that hears it, typically
    /// well under 300 ms).
    pub fn new(
        sample_rate: u32,
        frame_size: usize,
        filter_length: usize,
    ) -> Result<Self, AecError> {
        // SAFETY: FFI call with plain integer arguments; the returned
        // pointer is checked for null before use.
        let echo =
            unsafe { ffi::speex_echo_state_init(frame_size as c_int, filter_length as c_int) };
        let echo = ptr::NonNull::new(echo).ok_or(AecError::EchoInitFailed)?;

        // SAFETY: `echo` was just checked non-null; `rate` outlives the call.
        unsafe {
            let mut rate: c_int = sample_rate as c_int;
            ffi::speex_echo_ctl(
                echo.as_ptr(),
                ffi::SPEEX_ECHO_SET_SAMPLING_RATE,
                &mut rate as *mut c_int as *mut c_void,
            );
        }

        // SAFETY: FFI call with plain integer arguments; the returned
        // pointer is checked for null before use.
        let preprocess =
            unsafe { ffi::speex_preprocess_state_init(frame_size as c_int, sample_rate as c_int) };
        let preprocess = match ptr::NonNull::new(preprocess) {
            Some(state) => state,
            None => {
                // SAFETY: `echo` is a valid, still-owned state; this is the
                // only path that destroys it without returning `Self`.
                unsafe { ffi::speex_echo_state_destroy(echo.as_ptr()) };
                return Err(AecError::PreprocessInitFailed);
            }
        };

        // SAFETY: both pointers were just checked non-null; each `ptr`
        // argument outlives its `ctl` call. Attaching the echo state lets the
        // preprocessor suppress the residual echo the linear canceller
        // leaves behind; denoise stays off (this crate's job is separating
        // the two speakers, not general noise suppression).
        unsafe {
            ffi::speex_preprocess_ctl(
                preprocess.as_ptr(),
                ffi::SPEEX_PREPROCESS_SET_ECHO_STATE,
                echo.as_ptr() as *mut c_void,
            );
            let mut denoise: c_int = 0;
            ffi::speex_preprocess_ctl(
                preprocess.as_ptr(),
                ffi::SPEEX_PREPROCESS_SET_DENOISE,
                &mut denoise as *mut c_int as *mut c_void,
            );
            // Suppress a further 40 dB of anything the linear canceller
            // still classifies as echo, 15 dB while the near end is also
            // talking (double-talk) — speexdsp's own defaults, set
            // explicitly so a future speexdsp update can't silently change
            // this crate's aggressiveness.
            let mut echo_suppress: c_int = -40;
            ffi::speex_preprocess_ctl(
                preprocess.as_ptr(),
                ffi::SPEEX_PREPROCESS_SET_ECHO_SUPPRESS,
                &mut echo_suppress as *mut c_int as *mut c_void,
            );
            let mut echo_suppress_active: c_int = -15;
            ffi::speex_preprocess_ctl(
                preprocess.as_ptr(),
                ffi::SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE,
                &mut echo_suppress_active as *mut c_int as *mut c_void,
            );
        }

        Ok(EchoCanceller {
            echo,
            preprocess,
            frame_size,
        })
    }

    /// The frame length every `mic`/`reference`/`out` slice passed to
    /// [`cancel`](Self::cancel) must have.
    pub fn frame_size(&self) -> usize {
        self.frame_size
    }

    /// Cancels the echo of `reference` (what was played, e.g. the loopback
    /// channel) out of `mic` (what the microphone heard), writing the
    /// cleaned near-end signal to `out`. All three slices must be exactly
    /// [`frame_size`](Self::frame_size) samples.
    pub fn cancel(
        &mut self,
        mic: &[i16],
        reference: &[i16],
        out: &mut [i16],
    ) -> Result<(), AecError> {
        for len in [mic.len(), reference.len(), out.len()] {
            if len != self.frame_size {
                return Err(AecError::FrameSizeMismatch {
                    expected: self.frame_size,
                    actual: len,
                });
            }
        }

        // SAFETY: all three slices were just checked to hold exactly
        // `frame_size` `i16`s, matching what `speex_echo_cancellation` reads
        // from `rec`/`play` and writes to `out`.
        unsafe {
            ffi::speex_echo_cancellation(
                self.echo.as_ptr(),
                mic.as_ptr(),
                reference.as_ptr(),
                out.as_mut_ptr(),
            );
        }

        // SAFETY: `out` still holds exactly `frame_size` samples;
        // `speex_preprocess_run` processes it in place.
        unsafe {
            ffi::speex_preprocess_run(self.preprocess.as_ptr(), out.as_mut_ptr());
        }

        Ok(())
    }
}

impl Drop for EchoCanceller {
    fn drop(&mut self) {
        // SAFETY: both states are owned by `self` and destroyed exactly
        // once, here; the preprocessor referenced `self.echo` (set via
        // `SPEEX_PREPROCESS_SET_ECHO_STATE`) but does not own or outlive it,
        // so destroying the echo state second is fine either way.
        unsafe {
            ffi::speex_preprocess_state_destroy(self.preprocess.as_ptr());
            ffi::speex_echo_state_destroy(self.echo.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RATE: u32 = 16_000;
    const FRAME_SIZE: usize = 320; // 20 ms at 16 kHz
    const FILTER_LENGTH: usize = 4096; // 256 ms tail

    /// A cheap deterministic pseudo-random generator so tests don't depend
    /// on `rand` or the harness's banned `Math.random`-equivalent sources.
    fn lcg_noise(len: usize, seed: u32) -> Vec<i16> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((state >> 16) as i16) / 4 // keep well inside i16 range
            })
            .collect()
    }

    fn rms(samples: &[i16]) -> f64 {
        let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
        (sum_sq / samples.len() as f64).sqrt()
    }

    #[test]
    fn init_and_drop_does_not_panic() {
        let canceller = EchoCanceller::new(SAMPLE_RATE, FRAME_SIZE, FILTER_LENGTH).unwrap();
        assert_eq!(canceller.frame_size(), FRAME_SIZE);
    }

    #[test]
    fn frame_size_mismatch_is_rejected() {
        let mut canceller = EchoCanceller::new(SAMPLE_RATE, FRAME_SIZE, FILTER_LENGTH).unwrap();
        let short = vec![0i16; FRAME_SIZE - 1];
        let full = vec![0i16; FRAME_SIZE];
        let mut out = vec![0i16; FRAME_SIZE];
        let err = canceller.cancel(&short, &full, &mut out).unwrap_err();
        assert!(
            matches!(err, AecError::FrameSizeMismatch { expected: FRAME_SIZE, actual } if actual == FRAME_SIZE - 1)
        );
    }

    /// A pure echo (mic = attenuated, delayed copy of the reference, no
    /// distinct near-end signal) should adapt out over enough frames that
    /// the cancelled output's energy ends far below the untouched echo's.
    #[test]
    fn adapts_out_a_pure_echo() {
        let mut canceller = EchoCanceller::new(SAMPLE_RATE, FRAME_SIZE, FILTER_LENGTH).unwrap();

        let total_frames = 200; // 4s: several seconds is typical MDF convergence time
        let total_samples = total_frames * FRAME_SIZE;
        let reference = lcg_noise(total_samples, 42);

        // Echo path: 15 ms delay, attenuated by half — well inside the
        // configured 256 ms tail.
        let delay = 240usize; // 15ms at 16kHz
        let mut mic = vec![0i16; total_samples];
        for i in delay..total_samples {
            mic[i] = ((reference[i - delay] as i32) / 2) as i16;
        }

        let mut out = vec![0i16; total_samples];
        for frame in 0..total_frames {
            let range = frame * FRAME_SIZE..(frame + 1) * FRAME_SIZE;
            let mut frame_out = vec![0i16; FRAME_SIZE];
            canceller
                .cancel(
                    &mic[range.clone()],
                    &reference[range.clone()],
                    &mut frame_out,
                )
                .unwrap();
            out[range].copy_from_slice(&frame_out);
        }

        // Compare energy over the last second, by which point MDF has
        // converged, against the untouched echo's energy over the same
        // window.
        let tail = total_samples - SAMPLE_RATE as usize..total_samples;
        let echo_rms = rms(&mic[tail.clone()]);
        let out_rms = rms(&out[tail]);
        assert!(
            out_rms < echo_rms * 0.5,
            "expected adapted-out echo well below the original (echo_rms={echo_rms}, out_rms={out_rms})"
        );
    }

    /// A near-end signal with no echo at all should survive cancellation
    /// close to untouched (silent reference is speexdsp's natural no-op).
    #[test]
    fn distinct_near_end_survives_silent_reference() {
        let mut canceller = EchoCanceller::new(SAMPLE_RATE, FRAME_SIZE, FILTER_LENGTH).unwrap();

        let total_frames = 20;
        let total_samples = total_frames * FRAME_SIZE;
        let near_end = lcg_noise(total_samples, 7);
        let silence = vec![0i16; total_samples];

        let mut out = vec![0i16; total_samples];
        for frame in 0..total_frames {
            let range = frame * FRAME_SIZE..(frame + 1) * FRAME_SIZE;
            let mut frame_out = vec![0i16; FRAME_SIZE];
            canceller
                .cancel(
                    &near_end[range.clone()],
                    &silence[range.clone()],
                    &mut frame_out,
                )
                .unwrap();
            out[range].copy_from_slice(&frame_out);
        }

        let near_end_rms = rms(&near_end);
        let out_rms = rms(&out);
        assert!(
            out_rms > near_end_rms * 0.5,
            "expected the near-end signal to mostly survive a silent reference (near_end_rms={near_end_rms}, out_rms={out_rms})"
        );
    }
}
