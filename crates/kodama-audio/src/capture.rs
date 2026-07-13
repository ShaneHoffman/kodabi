//! Owns the `!Send` cpal `Stream` on a dedicated thread and exposes a
//! `Send`-safe handle (`Capture`) that Tauri can hold in managed state.
//! Device selection is the only thing that differs between capture sources
//! (see `CaptureSource`): WASAPI loopback works by building an *input*
//! stream on the default *output* device — cpal's WASAPI backend detects
//! the render endpoint and sets the loopback flag automatically — while the
//! microphone is a normal input stream on the default *input* device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{InputCallbackInfo, SampleFormat, SizedSample, StreamConfig};
use crossbeam_channel::{Receiver, Sender};

use crate::convert;
use crate::error::{AudioError, Result};
use crate::format::{AudioFormat, SampleTag};
use crate::frame::{AudioFrame, CaptureItem, SegmentReason};
use crate::meter::{LevelMeter, MeterSnapshot};
use crate::source::CaptureSource;

/// How long to wait before retrying a stream rebuild, so a rapid
/// device-switch storm (or a device still settling) doesn't spin.
const REBUILD_BACKOFF: Duration = Duration::from_millis(200);

/// How often a parked capture thread re-checks its control flags. The
/// real-time error callback must not touch the control lock/condvar, so it
/// only sets the `rebuild` flag; this poll interval bounds how long that
/// request waits to be noticed (a `stop()` still wakes the thread at once
/// via the condvar, since it runs off the audio thread).
const WAKE_POLL: Duration = Duration::from_millis(250);

struct Control {
    lock: Mutex<()>,
    cv: Condvar,
    stop: AtomicBool,
    rebuild: AtomicBool,
    running: AtomicBool,
}

impl Control {
    fn new() -> Self {
        Control {
            lock: Mutex::new(()),
            cv: Condvar::new(),
            stop: AtomicBool::new(false),
            rebuild: AtomicBool::new(false),
            running: AtomicBool::new(false),
        }
    }

    fn notify(&self) {
        let _guard = self.lock.lock().unwrap();
        self.cv.notify_all();
    }
}

enum WakeReason {
    Stop,
    Rebuild,
}

fn wait_for_wake(ctrl: &Control) -> WakeReason {
    let mut guard = ctrl.lock.lock().unwrap();
    loop {
        if ctrl.stop.load(Ordering::Acquire) {
            return WakeReason::Stop;
        }
        if ctrl.rebuild.load(Ordering::Acquire) {
            return WakeReason::Rebuild;
        }
        // Timed wait so a `rebuild` set from the RT error callback (which
        // never notifies) is still picked up within one poll interval; a
        // `stop()` notify wakes us immediately.
        let (next, _timeout) = ctrl.cv.wait_timeout(guard, WAKE_POLL).unwrap();
        guard = next;
    }
}

/// A running capture session for one source (system-audio loopback or the
/// microphone). Holds no `!Send` cpal types — the `cpal::Stream` lives
/// entirely on the dedicated capture thread — so this type is `Send` and
/// safe to keep in Tauri managed state.
pub struct Capture {
    ctrl: Arc<Control>,
    meter: Arc<LevelMeter>,
    items_rx: Receiver<CaptureItem>,
    format: AudioFormat,
    source: CaptureSource,
    handle: Option<JoinHandle<()>>,
}

impl Capture {
    /// Spawn the capture thread and block until the first segment is live,
    /// returning its negotiated format (or the error that prevented it).
    pub fn start(source: CaptureSource, item_capacity: usize) -> Result<Capture> {
        let ctrl = Arc::new(Control::new());
        let meter = Arc::new(LevelMeter::default());
        let (items_tx, items_rx) = crossbeam_channel::bounded(item_capacity);
        let (handshake_tx, handshake_rx) = mpsc::channel::<Result<AudioFormat>>();

        let thread_ctrl = ctrl.clone();
        let thread_meter = meter.clone();
        let handle = thread::Builder::new()
            .name(source.thread_name().to_string())
            .spawn(move || {
                capture_thread(source, thread_ctrl, thread_meter, items_tx, handshake_tx)
            })
            .map_err(|_| AudioError::ThreadStart)?;

        match handshake_rx.recv() {
            Ok(Ok(format)) => Ok(Capture {
                ctrl,
                meter,
                items_rx,
                format,
                source,
                handle: Some(handle),
            }),
            Ok(Err(err)) => {
                let _ = handle.join();
                Err(err)
            }
            Err(_) => {
                let _ = handle.join();
                Err(AudioError::ThreadStart)
            }
        }
    }

    /// Which source this session is capturing from.
    pub fn source(&self) -> CaptureSource {
        self.source
    }

    /// The format negotiated for the current segment.
    pub fn format(&self) -> AudioFormat {
        self.format
    }

    /// A cloned receiver for the item stream (segment markers + frames).
    /// The channel is MPMC: each item is delivered to exactly one receiver,
    /// so cloned receivers *compete* for items rather than each seeing the
    /// full stream. A consumer that needs to fan the stream out to several
    /// independent sinks (e.g. persistence *and* interleaving) must add its
    /// own broadcast layer — one clone per sink will not do it.
    pub fn items(&self) -> Receiver<CaptureItem> {
        self.items_rx.clone()
    }

    pub fn snapshot(&self) -> MeterSnapshot {
        self.meter.snapshot(self.is_running())
    }

    pub fn is_running(&self) -> bool {
        self.ctrl.running.load(Ordering::Acquire)
    }

    /// Stop capture and join the thread.
    pub fn stop(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.ctrl.stop.store(true, Ordering::Release);
        self.ctrl.notify();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Capture {
    // Guarantees the capture thread never outlives its handle, even if a
    // session is dropped without an explicit `stop()` call.
    fn drop(&mut self) {
        self.stop_inner();
    }
}

fn capture_thread(
    source: CaptureSource,
    ctrl: Arc<Control>,
    meter: Arc<LevelMeter>,
    items_tx: Sender<CaptureItem>,
    handshake_tx: mpsc::Sender<Result<AudioFormat>>,
) {
    let host = cpal::default_host();
    let mut segment_id: u32 = 0;
    let mut handshake_tx = Some(handshake_tx);

    loop {
        if ctrl.stop.load(Ordering::Acquire) {
            return;
        }

        match build_and_play(
            &host,
            source,
            segment_id,
            items_tx.clone(),
            meter.clone(),
            ctrl.clone(),
        ) {
            Ok((stream, format)) => {
                let reason = if segment_id == 0 {
                    SegmentReason::Start
                } else {
                    SegmentReason::DeviceChanged
                };
                // Best-effort: a saturated channel (no consumer draining
                // yet) drops the marker rather than blocking this thread.
                // Dropping it is non-fatal — every AudioFrame carries its
                // own format, so a consumer can still tell what applies to
                // the frames that follow (and detect the boundary from the
                // segment_id change).
                let _ = items_tx.try_send(CaptureItem::SegmentStarted {
                    segment_id,
                    format,
                    reason,
                });
                meter.inc_segments();
                // Publish `running` before the handshake: `start()` unblocks
                // the instant the handshake arrives and immediately reads
                // status, so this store must be visible by then or a live
                // session is reported as not-running.
                ctrl.running.store(true, Ordering::Release);
                if let Some(tx) = handshake_tx.take() {
                    let _ = tx.send(Ok(format));
                }

                let wake = wait_for_wake(&ctrl);
                drop(stream);
                ctrl.running.store(false, Ordering::Release);

                match wake {
                    WakeReason::Stop => return,
                    WakeReason::Rebuild => {
                        ctrl.rebuild.store(false, Ordering::Release);
                        segment_id += 1;
                        thread::sleep(REBUILD_BACKOFF);
                    }
                }
            }
            Err(err) => {
                if let Some(tx) = handshake_tx.take() {
                    let _ = tx.send(Err(err));
                    return; // no live session was ever established
                }
                // Mid-session rebuild attempt failed (device likely still
                // settling right after a switch) — back off and retry the
                // same segment rather than giving up on the whole session.
                thread::sleep(REBUILD_BACKOFF);
            }
        }
    }
}

fn build_and_play(
    host: &cpal::Host,
    source: CaptureSource,
    segment_id: u32,
    tx: Sender<CaptureItem>,
    meter: Arc<LevelMeter>,
    ctrl: Arc<Control>,
) -> Result<(cpal::Stream, AudioFormat)> {
    let (stream, format) = build_stream(host, source, segment_id, tx, meter, ctrl)?;
    stream.play().map_err(|e| AudioError::Play(e.to_string()))?;
    Ok((stream, format))
}

fn build_stream(
    host: &cpal::Host,
    source: CaptureSource,
    segment_id: u32,
    tx: Sender<CaptureItem>,
    meter: Arc<LevelMeter>,
    ctrl: Arc<Control>,
) -> Result<(cpal::Stream, AudioFormat)> {
    let (device, supported) = source.resolve(host)?;

    let sample_format = supported.sample_format();
    let format = AudioFormat {
        sample_rate: supported.sample_rate(),
        channels: supported.channels(),
        source_format: SampleTag::from(sample_format),
    };
    let config: StreamConfig = supported.config();

    let error_callback = move |err: cpal::Error| {
        if is_device_change(&err) {
            // Set the flag only — do NOT touch the control lock or condvar
            // here, since cpal may invoke this callback on the real-time
            // audio thread. The capture thread's parked wait polls this flag
            // (see WAKE_POLL) and rebuilds within one poll interval.
            ctrl.rebuild.store(true, Ordering::Release);
        }
        // Other errors (e.g. transient xruns) are non-fatal — swallow them
        // rather than ever panic in the RT error path.
    };

    let stream = match sample_format {
        SampleFormat::F32 => build_typed_stream::<f32>(
            &device,
            config,
            segment_id,
            format,
            tx,
            meter,
            error_callback,
        ),
        SampleFormat::I16 => build_typed_stream::<i16>(
            &device,
            config,
            segment_id,
            format,
            tx,
            meter,
            error_callback,
        ),
        SampleFormat::U16 => build_typed_stream::<u16>(
            &device,
            config,
            segment_id,
            format,
            tx,
            meter,
            error_callback,
        ),
        SampleFormat::I32 => build_typed_stream::<i32>(
            &device,
            config,
            segment_id,
            format,
            tx,
            meter,
            error_callback,
        ),
        other => return Err(AudioError::UnsupportedFormat(other)),
    }
    .map_err(|e| AudioError::BuildStream(e.to_string()))?;

    Ok((stream, format))
}

fn is_device_change(err: &cpal::Error) -> bool {
    matches!(
        err.kind(),
        cpal::ErrorKind::DeviceNotAvailable
            | cpal::ErrorKind::DeviceChanged
            | cpal::ErrorKind::StreamInvalidated
    )
}

fn build_typed_stream<T>(
    device: &cpal::Device,
    config: StreamConfig,
    segment_id: u32,
    format: AudioFormat,
    tx: Sender<CaptureItem>,
    meter: Arc<LevelMeter>,
    error_callback: impl FnMut(cpal::Error) + Send + 'static,
) -> std::result::Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: cpal::FromSample<T>,
{
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _: &InputCallbackInfo| {
            let samples = convert::to_f32(data);
            meter.observe(&samples);
            let frame = AudioFrame {
                segment_id,
                samples,
                format,
            };
            if tx.try_send(CaptureItem::Frame(frame)).is_err() {
                meter.inc_dropped();
            }
        },
        error_callback,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_for_wake_returns_stop_when_stop_flag_set() {
        let ctrl = Arc::new(Control::new());
        let waiter_ctrl = ctrl.clone();
        let handle = thread::spawn(move || wait_for_wake(&waiter_ctrl));

        thread::sleep(Duration::from_millis(50));
        ctrl.stop.store(true, Ordering::Release);
        ctrl.notify();

        let reason = handle.join().unwrap();
        assert!(matches!(reason, WakeReason::Stop));
    }

    #[test]
    fn wait_for_wake_returns_rebuild_when_rebuild_flag_set() {
        let ctrl = Arc::new(Control::new());
        let waiter_ctrl = ctrl.clone();
        let handle = thread::spawn(move || wait_for_wake(&waiter_ctrl));

        thread::sleep(Duration::from_millis(50));
        ctrl.rebuild.store(true, Ordering::Release);
        ctrl.notify();

        let reason = handle.join().unwrap();
        assert!(matches!(reason, WakeReason::Rebuild));
    }
}
