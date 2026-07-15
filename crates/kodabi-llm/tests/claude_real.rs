//! Integration test that drives a real headless `claude` subprocess and
//! proves the Done-when for this ticket: a transcript with mangled glossary
//! terms comes back corrected through [`ClaudeCleaner`].
//!
//! `#[ignore]` because it spends a small amount of real Claude usage and
//! requires a working, authenticated `claude` CLI on `PATH` (subscription
//! login or `ANTHROPIC_API_KEY` — see `kodabi_llm`'s crate docs). Run with:
//!
//! ```text
//! cargo test -p kodabi-llm --test claude_real -- --ignored
//! ```

use kodabi_core::glossary::{Glossary, GlossaryTerm, OnConflict};
use kodabi_core::raw_session::TranscriptSegment;
use kodabi_core::transcription::{clean_transcript, Channel};
use kodabi_llm::{ClaudeCleaner, ClaudeConfig};

fn segment(index: u64, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        index,
        channel: Channel::You,
        speaker: None,
        start_ms: index * 1000,
        end_ms: index * 1000 + 500,
        text: text.to_owned(),
    }
}

fn term(term: &str, definition: &str, aliases: &[&str]) -> GlossaryTerm {
    GlossaryTerm {
        term: term.to_owned(),
        definition: definition.to_owned(),
        aliases: aliases.iter().map(|s| s.to_string()).collect(),
    }
}

fn meridian_and_teetrack_glossary() -> Glossary {
    let mut glossary = Glossary::default();
    glossary
        .upsert(
            term("MERIDIAN", "A regional systems-migration project.", &["meridian"]),
            OnConflict::Error,
        )
        .expect("upsert succeeds");
    glossary
        .upsert(
            term("TeeTrack", "Tee-sheet / POS vendor.", &[]),
            OnConflict::Error,
        )
        .expect("upsert succeeds");
    glossary
}

/// Same segments regardless of which engine produced them (Parakeet or
/// Whisper) — the post-pass runs on assembled [`TranscriptSegment`] text, so
/// this is exactly what makes it engine-agnostic.
#[test]
#[ignore = "spawns a real headless `claude` process and spends a small amount of real usage"]
fn corrects_mangled_glossary_terms_via_a_real_headless_claude_call() {
    let glossary = meridian_and_teetrack_glossary();
    let segments = vec![
        segment(0, "lets sync up on the mare idian project tomorrow"),
        segment(1, "did tea track ship the new release yet"),
        segment(2, "how was your weekend"),
    ];

    let cleaner = ClaudeCleaner::new(ClaudeConfig::default());
    let cleaned = clean_transcript(&cleaner, segments.clone(), &glossary);

    assert!(
        cleaned[0].text.contains("MERIDIAN"),
        "expected the MERIDIAN misrecognition corrected, got: {:?}",
        cleaned[0].text
    );
    assert!(
        cleaned[1].text.contains("TeeTrack"),
        "expected the TeeTrack misrecognition corrected, got: {:?}",
        cleaned[1].text
    );
    assert_eq!(
        cleaned[2], segments[2],
        "a segment with no glossary term should pass through untouched"
    );
}

#[test]
#[ignore = "spawns a real headless `claude` process and spends a small amount of real usage"]
fn leaves_a_transcript_with_no_relevant_terms_unchanged() {
    let glossary = meridian_and_teetrack_glossary();
    let segments = vec![segment(0, "how was your weekend, did you get any rest")];

    let cleaner = ClaudeCleaner::new(ClaudeConfig::default());
    let cleaned = clean_transcript(&cleaner, segments.clone(), &glossary);

    assert_eq!(cleaned, segments);
}
