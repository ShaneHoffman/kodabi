//! Reconciles the two per-engine benchmark runs (`benchmark_parakeet.rs`,
//! `benchmark_whisper.rs`) into a single comparison. The two engines can't
//! be linked into one binary (see `Cargo.toml`'s `parakeet`/`whisper`
//! feature docs on the static/shared sherpa-onnx link conflict), so each
//! writes its own `results_<engine>.json` into the fixture directory, and
//! this feature-free test reads both back and prints the comparison.
//!
//! `#[ignore]`d because it needs both engines' benchmark runs to have
//! already produced `results_parakeet.json` and `results_whisper.json` in
//! `KODABI_BENCHMARK_MEETING_DIR`. Deliberately has no `#![cfg(feature =
//! ...)]` — it never touches either native engine, only the two JSON result
//! files — so it compiles under the crate's default (feature-free) build.
//!
//! ```text
//! KODABI_BENCHMARK_MEETING_DIR=... cargo test -p kodabi-transcribe -- --ignored --nocapture benchmark_compare
//! ```

use std::path::PathBuf;

use kodabi_core::benchmark::{compare_engines, read_engine_score, render_markdown};

fn fixture_dir() -> Option<PathBuf> {
    std::env::var("KODABI_BENCHMARK_MEETING_DIR")
        .ok()
        .map(PathBuf::from)
}

#[test]
#[ignore = "requires both benchmark_parakeet and benchmark_whisper to have already run and written results_*.json into KODABI_BENCHMARK_MEETING_DIR"]
fn benchmark_compare_prints_the_engine_comparison() {
    let dir = fixture_dir().expect("set KODABI_BENCHMARK_MEETING_DIR to run this comparison");

    let parakeet = read_engine_score(&dir.join("results_parakeet.json"))
        .expect("run benchmark_parakeet first to produce results_parakeet.json");
    let whisper = read_engine_score(&dir.join("results_whisper.json"))
        .expect("run benchmark_whisper first to produce results_whisper.json");

    let comparison = compare_engines(vec![parakeet, whisper]);
    println!("{}", render_markdown(&comparison));

    assert!(comparison.suggested_winner.is_some());
}
