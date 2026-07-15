use kodabi_audio::{Capture, CaptureSource};

/// Manual verification of the ticket's "Done when": capture a few seconds of
/// whatever the speakers are playing and confirm non-silent samples arrive.
/// Requires a real default output device with audio playing, so it's not
/// run in CI:
///
///   cargo test -p kodabi-audio -- --ignored --nocapture
#[test]
#[ignore = "requires a real output device with audio playing (WASAPI loopback)"]
fn loopback_captures_nonzero_audio() {
    let capture =
        Capture::start(CaptureSource::Loopback, 256).expect("failed to start loopback capture");

    let format = capture.format();
    assert!(format.sample_rate >= 8_000, "unexpectedly low sample rate");
    assert!(format.channels >= 1, "expected at least one channel");

    std::thread::sleep(std::time::Duration::from_secs(3));

    let snapshot = capture.snapshot();
    capture.stop();

    assert!(
        snapshot.frames_captured > 0,
        "no frames arrived from loopback"
    );
    assert!(
        snapshot.peak > 0.0,
        "captured only silence — was audio playing on the default output device?"
    );
}
