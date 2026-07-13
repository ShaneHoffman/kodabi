use std::time::Duration;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use kodama_audio::{Capture, CaptureSource, Combiner, SessionChannel};

/// Direct verification of this ticket's "Done when": mic and loopback
/// capture in parallel, get combined by `Combiner` into one time-aligned
/// two-channel session (channel 0 = mic = "you", channel 1 = system =
/// "them"), and round-trip through a stereo WAV — the tangible two-channel
/// artifact. Requires a real default output *and* input device, with audio
/// actually playing and someone speaking into the mic, so it's not run in
/// CI:
///
///   cargo test -p kodama-audio -- --ignored --nocapture
#[test]
#[ignore = "requires real output + input devices (loopback + microphone), with audio playing"]
fn two_channel_session_is_aligned_and_round_trips_through_wav() {
    let loopback =
        Capture::start(CaptureSource::Loopback, 256).expect("failed to start loopback capture");
    let microphone =
        Capture::start(CaptureSource::Microphone, 256).expect("failed to start microphone capture");

    let combiner = Combiner::start(microphone.items(), loopback.items(), 48_000)
        .expect("failed to start combiner");

    let capture_duration = Duration::from_secs(3);
    std::thread::sleep(capture_duration);

    // Both captures must stop (and their frame-channel senders drop)
    // before `finish()` can observe disconnection and return.
    loopback.stop();
    microphone.stop();
    let session = combiner.finish();

    assert_eq!(session.sample_rate(), 48_000);

    let mic = session.channel(SessionChannel::Mic);
    let system = session.channel(SessionChannel::System);
    assert_eq!(
        mic.len(),
        system.len(),
        "both channels must end up equally long"
    );
    assert!(session.frames() > 0, "combiner produced no aligned audio");

    let expected_secs = capture_duration.as_secs_f64();
    let actual_secs = session.duration().as_secs_f64();
    assert!(
        (actual_secs - expected_secs).abs() < 1.0,
        "aligned session duration ({actual_secs}s) should track the ~{expected_secs}s capture window"
    );

    assert!(
        system.iter().any(|&s| s.abs() > 0.0),
        "captured only silence on the system channel — was audio playing on the default output device?"
    );

    // Round-trip through a stereo WAV: L = mic/you, R = system/them. This
    // stays test-only for v1 — runtime persistence and retention are
    // `feat/persist-raw-session`'s concern, not this ticket's.
    let dir = tempfile::tempdir().expect("failed to create scratch dir");
    let path = dir.path().join("two-channel-session.wav");
    let spec = WavSpec {
        channels: 2,
        sample_rate: session.sample_rate(),
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer = WavWriter::create(&path, spec).expect("failed to create wav writer");
    for sample in session.interleaved_stereo() {
        writer.write_sample(sample).expect("failed to write sample");
    }
    writer.finalize().expect("failed to finalize wav file");

    let reader = WavReader::open(&path).expect("failed to open written wav file");
    let read_spec = reader.spec();
    assert_eq!(read_spec.channels, 2);
    assert_eq!(read_spec.sample_rate, session.sample_rate());
    assert_eq!(
        reader.duration() as usize,
        session.frames(),
        "wav frame count should match the aligned session"
    );
}
