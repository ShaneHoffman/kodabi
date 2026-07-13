use kodama_audio::{Capture, CaptureSource};

/// Manual verification that the microphone path negotiates and captures
/// frames. Frames flow regardless of level, so this doesn't require anyone
/// to speak — it just needs a real default input device. Not run in CI:
///
///   cargo test -p kodama-audio -- --ignored --nocapture
#[test]
#[ignore = "requires a real default input device (microphone)"]
fn microphone_captures_frames() {
    let capture =
        Capture::start(CaptureSource::Microphone, 256).expect("failed to start microphone capture");

    let format = capture.format();
    assert!(format.sample_rate >= 8_000, "unexpectedly low sample rate");
    assert!(format.channels >= 1, "expected at least one channel");

    std::thread::sleep(std::time::Duration::from_secs(3));

    let snapshot = capture.snapshot();
    capture.stop();

    assert!(
        snapshot.frames_captured > 0,
        "no frames arrived from the microphone"
    );
}
