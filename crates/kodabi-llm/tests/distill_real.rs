//! Integration test that drives a real headless `claude` subprocess and
//! proves this ticket's Done-when: a stored transcript yields a schema-valid
//! meeting note with summary, decisions, and owned action items.
//!
//! `#[ignore]` because it spends real Claude usage (a distill-sized call on
//! the distill default model) and requires a working, authenticated `claude`
//! CLI on `PATH` (subscription login or `ANTHROPIC_API_KEY` — see
//! `kodabi_llm`'s crate docs). Run with:
//!
//! ```text
//! cargo test -p kodabi-llm --test distill_real -- --ignored
//! ```

use chrono::{TimeZone, Utc};
use kodabi_core::device::DeviceId;
use kodabi_core::distill::{distill_session, inbox_routing};
use kodabi_core::note::{Note, NoteType, INBOX};
use kodabi_core::raw_session::{write_raw_session, TranscriptSegment};
use kodabi_core::transcription::Channel;
use kodabi_llm::{ClaudeConfig, ClaudeRunner};

fn segment(index: u64, channel: Channel, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        index,
        channel,
        speaker: None,
        start_ms: index * 5_000,
        end_ms: index * 5_000 + 4_000,
        text: text.to_owned(),
    }
}

/// A small fabricated meeting with an unambiguous decision and two owned,
/// dated commitments — enough signal that any capable model extracts them.
fn meeting_segments() -> Vec<TranscriptSegment> {
    [
        (
            Channel::You,
            "okay, last thing on the agenda: the irrigation contract for the course renovation",
        ),
        (
            Channel::Them,
            "we compared the three bids again this morning",
        ),
        (
            Channel::You,
            "right, and given the numbers I think we should go with GreenFlow Systems",
        ),
        (
            Channel::Them,
            "agreed, let's make it official: GreenFlow is our irrigation contractor",
        ),
        (
            Channel::You,
            "great. I will send the signed contract to legal by July 20th, 2026",
        ),
        (
            Channel::Them,
            "and I'll schedule the kickoff call with GreenFlow for early August",
        ),
        (
            Channel::You,
            "one thing we still don't know is whether the west course closes during the work",
        ),
        (
            Channel::Them,
            "yeah, that's still open, we need facilities to weigh in",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (channel, text))| segment(i as u64, channel, text))
    .collect()
}

#[test]
#[ignore = "spawns a real headless `claude` process and spends real usage on the distill model"]
fn distills_a_stored_transcript_into_a_schema_valid_meeting_note() {
    let vault = tempfile::tempdir().expect("tempdir");
    let captured_at = Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap();
    let device = DeviceId::parse("k4m2xp7q").unwrap();
    let session_path = write_raw_session(
        &vault.path().join("sessions"),
        captured_at,
        &device,
        Some("irrigation sync"),
        &meeting_segments(),
    )
    .expect("session should persist");

    let runner = ClaudeRunner::new(ClaudeConfig::distill());
    let distilled = distill_session(
        &runner,
        vault.path(),
        &session_path,
        &|_, _| inbox_routing(),
        &no_open_entries,
    )
    .expect("distill should succeed");

    let written = std::fs::read_to_string(&distilled.path).expect("note file should exist");
    let note = Note::from_markdown(&written).expect("note should be schema-valid");

    assert_eq!(note.note_type, NoteType::Meeting);
    assert_eq!(note.routing.project(), INBOX);
    assert_eq!(note.routing.confidence(), Some(0.0));
    assert!(note.body.starts_with("# Summary"), "body: {}", note.body);
    assert!(
        note.body.contains("## Decisions") && note.body.to_lowercase().contains("greenflow"),
        "expected the GreenFlow decision extracted, body: {}",
        note.body
    );
    assert!(note.body.contains("## Action items"), "body: {}", note.body);

    // Every rendered action item must match the documented line grammar:
    // "- [ ] {Owner} to {description}[ by YYYY-MM-DD]." — and at least one
    // must be a real owned item (not Unassigned), since both commitments in
    // the transcript have explicit owners.
    let action_lines: Vec<&str> = note
        .body
        .lines()
        .skip_while(|line| *line != "## Action items")
        .skip(1)
        .take_while(|line| !line.starts_with("## "))
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert!(
        !action_lines.is_empty(),
        "no action items in: {}",
        note.body
    );
    for line in &action_lines {
        let rest = line
            .strip_prefix("- [ ] ")
            .unwrap_or_else(|| panic!("not a checkbox line: {line}"));
        let rest = rest
            .strip_suffix('.')
            .unwrap_or_else(|| panic!("no terminal period: {line}"));
        assert!(rest.contains(" to "), "no owner split in: {line}");
    }
    assert!(
        action_lines
            .iter()
            .any(|line| !line.starts_with("- [ ] Unassigned to ")),
        "expected at least one owned action item, got: {action_lines:?}"
    );
}

/// The fetcher for a distill with no ledger behind it: this test exercises the
/// real model against the note pipeline, not the commitment classifications.
fn no_open_entries(
    _: &kodabi_core::routing::RouteGuess,
) -> Vec<kodabi_core::distill::OpenCommitment> {
    Vec::new()
}
