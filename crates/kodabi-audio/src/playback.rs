//! Minimal cpal output playback: play a provided mono `f32` buffer through
//! the default output device once, blocking until it finishes. This crate's
//! only playback path — capture is the focus everywhere else — built for
//! `mic_test`'s calibration tone, not as a general-purpose audio player.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::error::{AudioError, Result};
use crate::resample::MonoResampler;

/// How long to keep polling for the playback callback to finish before
/// giving up — a stuck or disconnected device must not hang the caller
/// forever (the mic test's whole point is to run and report, even on a
/// broken audio setup).
const PLAYBACK_TIMEOUT: Duration = Duration::from_secs(30);

/// A little silence after the last real sample is handed to the callback, so
/// the device's own output buffering finishes rendering it rather than the
/// stream being torn down mid-render.
const DRAIN_TAIL: Duration = Duration::from_millis(150);

/// Plays `mono` (sampled at `sample_rate`) through the default output
/// device, resampling to the device's negotiated rate first if they differ,
/// and blocks until playback completes.
pub fn play_blocking(mono: &[f32], sample_rate: u32) -> Result<()> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or(AudioError::NoDefaultOutputDevice)?;
    let supported = device
        .default_output_config()
        .map_err(|err| AudioError::DefaultOutputConfig(err.to_string()))?;

    let device_rate = supported.sample_rate();
    let mono_at_device_rate = if device_rate == sample_rate {
        mono.to_vec()
    } else {
        let mut resampler = MonoResampler::new(sample_rate, device_rate)?;
        let mut out = resampler.push(mono);
        out.extend(resampler.flush());
        out
    };

    let channels = supported.channels();
    let mut interleaved = Vec::with_capacity(mono_at_device_rate.len() * channels as usize);
    for &sample in &mono_at_device_rate {
        interleaved.extend(std::iter::repeat_n(sample, channels as usize));
    }

    let config: cpal::StreamConfig = supported.config();
    let sample_format = supported.sample_format();
    let position = Arc::new(AtomicUsize::new(0));
    let done = Arc::new(AtomicBool::new(false));

    let stream = match sample_format {
        SampleFormat::F32 => {
            build_output_stream::<f32>(&device, config, interleaved, position, done.clone())
        }
        SampleFormat::I16 => {
            build_output_stream::<i16>(&device, config, interleaved, position, done.clone())
        }
        SampleFormat::U16 => {
            build_output_stream::<u16>(&device, config, interleaved, position, done.clone())
        }
        SampleFormat::I32 => {
            build_output_stream::<i32>(&device, config, interleaved, position, done.clone())
        }
        other => return Err(AudioError::UnsupportedFormat(other)),
    }?;
    stream
        .play()
        .map_err(|err| AudioError::Play(err.to_string()))?;

    let deadline = Instant::now() + PLAYBACK_TIMEOUT;
    while !done.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(DRAIN_TAIL);
    Ok(())
}

/// Builds and plays an output stream of native type `T`, writing
/// `interleaved` once and then silence, flagging `done` once every real
/// sample has been handed to the device.
fn build_output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    interleaved: Vec<f32>,
    position: Arc<AtomicUsize>,
    done: Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let start = position.load(Ordering::Acquire);
                let remaining = interleaved.len().saturating_sub(start);
                let to_copy = remaining.min(data.len());
                for (i, slot) in data.iter_mut().enumerate() {
                    let sample = if i < to_copy {
                        interleaved[start + i]
                    } else {
                        0.0
                    };
                    *slot = T::from_sample(sample);
                }
                let new_position = start + to_copy;
                position.store(new_position, Ordering::Release);
                if new_position >= interleaved.len() {
                    done.store(true, Ordering::Release);
                }
            },
            |err| eprintln!("kodabi-audio: playback stream error: {err}"),
            None,
        )
        .map_err(|err| AudioError::BuildStream(err.to_string()))
}
