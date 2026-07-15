use kodabi_audio::{Capture, CaptureSource};

/// Direct verification of this ticket's "Done when": mic and loopback
/// capture simultaneously and both PCM streams are available for the same
/// session. Requires a real default output *and* input device, so it's not
/// run in CI:
///
///   cargo test -p kodabi-audio -- --ignored --nocapture
#[test]
#[ignore = "requires real default output and input devices (loopback + microphone)"]
fn loopback_and_microphone_capture_in_parallel() {
    let loopback =
        Capture::start(CaptureSource::Loopback, 256).expect("failed to start loopback capture");
    let microphone =
        Capture::start(CaptureSource::Microphone, 256).expect("failed to start microphone capture");

    assert!(loopback.is_running());
    assert!(microphone.is_running());

    std::thread::sleep(std::time::Duration::from_secs(2));

    let loopback_snapshot = loopback.snapshot();
    let microphone_snapshot = microphone.snapshot();
    loopback.stop();
    microphone.stop();

    assert!(
        loopback_snapshot.frames_captured > 0,
        "no frames arrived from loopback while capturing in parallel"
    );
    assert!(
        microphone_snapshot.frames_captured > 0,
        "no frames arrived from the microphone while capturing in parallel"
    );
}
