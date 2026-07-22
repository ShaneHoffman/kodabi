use kodabi_audio::run_mic_test;

/// Manual verification that the mic test runs end to end against real
/// hardware: plays the tone, records the mic, and returns a classification.
/// This can't assert *which* outcome — that depends on the machine running
/// it (headphones vs. speakers) — only that the whole pipeline completes.
/// Not run in CI:
///
///   cargo test -p kodabi-audio -- --ignored --nocapture
#[test]
#[ignore = "requires a real default input and output device"]
fn mic_test_runs_and_classifies() {
    let outcome = run_mic_test().expect("mic test failed to run");
    println!("mic test outcome: {outcome:?}");
}
