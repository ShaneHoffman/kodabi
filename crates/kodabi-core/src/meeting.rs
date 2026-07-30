//! Structured facts derived from a distilled note body, for the MCP `get_note`
//! tool's `MeetingMeta` + `ActionItem` output and the outstanding-items surface.
//!
//! A distilled note stores its decisions and action items only as Markdown in its
//! body (the distill pass renders them and keeps no structured copy — see
//! [`crate::distill`]'s action-item grammar), and its duration / speaker count
//! nowhere at all: those live only in the raw session transcript the note's
//! `source` points at. This module re-derives all of that from disk so the index
//! can cache it (`crate::index`) and `get_note` can serve it without re-parsing
//! the body or re-reading the JSONL per call.
//!
//! **Scope: meeting *and* chat notes** ([`derives_facts`]) — "chats are documents
//! too" (FOUNDING_DOC §3.6), and a chat's commitments are as real as a meeting's.
//! The `meeting` names here (this module, [`MeetingFacts`], the `note_meetings`
//! table, `IndexedNote::meeting`) are historical: they predate the chat leg and
//! were kept because the MCP wire object is still `meeting`/`MeetingMeta`, which
//! stays meeting-only. The two session-derived scalars are the real divergence —
//! they are always `None` for a chat, whose `source` is a chat transcript rather
//! than a session recording.
//!
//! Everything here is a pure function of the note's body + its session file, so a
//! reindex reproduces it exactly — the index stays a rebuildable cache
//! (FOUNDING_DOC §3.6). In particular an action item's id is a deterministic hash
//! of its content (below), never a random mint, so it is stable across reindexes.

use std::collections::HashMap;
use std::path::Path;

use crate::distill::{parse_action_line, UNASSIGNED_OWNER};
use crate::note::{Note, NoteType, Source};
use crate::raw_session::{self, TranscriptSegment};
use crate::transcription::Channel;

/// The structured facts a distilled note carries: the two session-derived scalars
/// (`None` when there is no measurable transcript), the ordered decisions, and
/// the ordered action items.
///
/// Historical name — this is also a chat note's facts (see the module doc). Both
/// scalars are always `None` for a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingFacts {
    /// Transcript length in whole seconds, or `None` when the `source` is a
    /// keyword (e.g. `manual`), the session file is gone (retention-pruned), or
    /// the note is a chat (never measured).
    pub duration_seconds: Option<u32>,
    /// Count of distinct real speaker channels (`You`/`Them`) present in the
    /// transcript; `None` when unknown (keyword/pruned source, an unattributed
    /// transcript, or a chat). Names are not resolved in v1.
    pub speaker_count: Option<u32>,
    /// The `## Decisions` bullets, in body order.
    pub decisions: Vec<String>,
    /// The `## Action items` lines, in body order.
    pub action_items: Vec<ActionItemFact>,
}

/// One extracted action item — the durable shape the distill draft
/// ([`crate::distill`]'s `ActionItemDraft`) is minted into: a stable id, the
/// parsed grammar fields, and the checkbox state. `overdue` is *not* here — it is
/// derived server-side from `done` + `due_date` against today, at read time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItemFact {
    /// Stable `^a_[0-9a-z]{6,}$` id — a deterministic hash of the note id and the
    /// line's content (see [`action_item_id`]).
    pub id: String,
    pub description: String,
    /// Never empty: [`UNASSIGNED_OWNER`] when the line attributed no owner.
    pub owner: String,
    /// `YYYY-MM-DD`, or `None` when the line carried no due date.
    pub due_date: Option<String>,
    /// Whether the checkbox was checked (`- [x]`). Maps to `done`; an unchecked
    /// box is `open` (and possibly `overdue`, derived later).
    pub done: bool,
    /// The meeting's own calendar day (from the note `date`), or `None` when the
    /// date is unparseable.
    pub extracted_date: Option<String>,
}

/// Whether a note of this type carries facts worth deriving — the single source
/// of truth for the gate, mirrored in SQL by
/// [`note_ids_missing_meeting_facts`](crate::index::NoteIndex::note_ids_missing_meeting_facts).
///
/// The rule is *"is this body machine-rendered by [`crate::distill::render_body`]?"*
/// [`parse_action_line`] is documented as that renderer's exact inverse, so a
/// meeting body and a chat body round-trip through the grammar by construction.
///
/// [`NoteType::Note`] is deliberately excluded: `quick_capture`
/// ([`crate::capture`]) writes the user's text verbatim and never runs the distill
/// pass, so nothing guarantees a `note` body follows the grammar. Since the parser
/// takes everything before the *first* `" to "` as the owner, a hand-written
/// `- [ ] Send the deck to Priya.` would index with owner `"Send the deck"` and
/// description `"Priya"`. Hand-written commitments belong to the Phase 5
/// commitment ledger, which can widen this deliberately.
pub fn derives_facts(note_type: NoteType) -> bool {
    matches!(note_type, NoteType::Meeting | NoteType::Chat)
}

/// Derives facts for a note, or `None` when its type carries none
/// ([`derives_facts`]).
///
/// The convenience entry point for the index write path, which holds a parsed
/// [`Note`]. The backfill path holds only a stored row, so it parses the pieces
/// out and calls [`derive_meeting_facts`] directly.
pub fn meeting_facts_for(note: &Note, kb_root: &Path) -> Option<MeetingFacts> {
    derives_facts(note.note_type).then(|| {
        derive_meeting_facts(
            note.id.as_str(),
            note.note_type,
            &note.date,
            &note.source,
            &note.body,
            kb_root,
        )
    })
}

/// Derives the structured facts from a distilled note's primitive fields. Takes
/// the pieces rather than a `&Note` so both the write path (from a parsed note)
/// and the backfill pass (from a stored `NoteRow`) reuse it. The caller is
/// responsible for only invoking it on a type [`derives_facts`] accepts.
///
/// `kb_root` resolves a `RawArtifact` `source` (a repo-relative path) to the
/// session JSONL; a keyword source or a missing/pruned file yields `None`
/// duration and speaker count.
///
/// `note_type` gates the session read: only a meeting's `source` points at a
/// session recording. A chat's points at its `chats/*.jsonl` transcript, whose
/// records are `ChatRecord`s, not `TranscriptSegment`s — reading it would slurp
/// the whole file only to fail deserializing it, on every index and every
/// backfill. Both scalars are `None` for a chat by construction, not by accident.
pub fn derive_meeting_facts(
    note_id: &str,
    note_type: NoteType,
    date: &str,
    source: &Source,
    body: &str,
    kb_root: &Path,
) -> MeetingFacts {
    let (decisions, action_items) = parse_body(note_id, date, body);
    let (duration_seconds, speaker_count) = if note_type == NoteType::Meeting {
        session_metrics(source, kb_root)
    } else {
        (None, None)
    };
    MeetingFacts {
        duration_seconds,
        speaker_count,
        decisions,
        action_items,
    }
}

/// Which body section a line currently belongs to as the parser walks the note.
enum Section {
    Other,
    Decisions,
    ActionItems,
}

/// Extracts the `## Decisions` bullets and `## Action items` lines from a note
/// body. A `##` header switches the active section; any other `#`-header (e.g.
/// `# Summary`) resets to `Other`, so only lines under the two named sections are
/// collected. Unparseable action lines are skipped.
fn parse_body(note_id: &str, date: &str, body: &str) -> (Vec<String>, Vec<ActionItemFact>) {
    let extracted_date = date.get(..10).and_then(crate::distill::valid_iso_date);

    let mut decisions = Vec::new();
    let mut action_items = Vec::new();
    // Occurrences of each identical (owner, description, due) tuple seen so far,
    // so duplicate lines get distinct-but-stable ids.
    let mut occurrences: HashMap<(String, String, Option<String>), u32> = HashMap::new();
    let mut section = Section::Other;

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if let Some(heading) = line.strip_prefix("## ") {
            section = match heading.trim() {
                "Decisions" => Section::Decisions,
                "Action items" => Section::ActionItems,
                _ => Section::Other,
            };
            continue;
        }
        if line.starts_with("# ") {
            section = Section::Other;
            continue;
        }
        match section {
            Section::Decisions => {
                if let Some(item) = line.strip_prefix("- ") {
                    let item = item.trim();
                    if !item.is_empty() {
                        decisions.push(item.to_string());
                    }
                }
            }
            Section::ActionItems => {
                if let Some((owner, description, due_date)) = parse_action_line(line) {
                    let owner = if owner.trim().is_empty() {
                        UNASSIGNED_OWNER.to_string()
                    } else {
                        owner
                    };
                    let key = (owner.clone(), description.clone(), due_date.clone());
                    let occurrence = occurrences.entry(key).or_insert(0);
                    let id = action_item_id(
                        note_id,
                        &owner,
                        &description,
                        due_date.as_deref(),
                        *occurrence,
                    );
                    *occurrence += 1;
                    action_items.push(ActionItemFact {
                        id,
                        description,
                        owner,
                        due_date,
                        done: line.starts_with("- [x] "),
                        extracted_date: extracted_date.clone(),
                    });
                }
            }
            Section::Other => {}
        }
    }

    (decisions, action_items)
}

/// Reads the session transcript (if any) and computes `(duration_seconds,
/// speaker_count)`. A keyword source, or a `RawArtifact` whose file is gone,
/// yields `(None, None)` — both `MeetingMeta` fields are nullable for exactly
/// this case.
fn session_metrics(source: &Source, kb_root: &Path) -> (Option<u32>, Option<u32>) {
    let Source::RawArtifact(rel) = source else {
        return (None, None);
    };
    let Ok(segments) = raw_session::read_raw_session(&kb_root.join(rel)) else {
        return (None, None);
    };
    (duration_seconds(&segments), speaker_count(&segments))
}

/// Whole-second transcript length ≈ the last segment's end offset. `None` for an
/// empty transcript.
fn duration_seconds(segments: &[TranscriptSegment]) -> Option<u32> {
    segments
        .iter()
        .map(|segment| segment.end_ms)
        .max()
        .map(|ms| (ms / 1000) as u32)
}

/// Distinct real channels (`You`/`Them`) present; `Unknown` is not a resolvable
/// speaker, so a transcript with only unknown (or no) segments yields `None`.
fn speaker_count(segments: &[TranscriptSegment]) -> Option<u32> {
    let saw_you = segments.iter().any(|s| s.channel == Channel::You);
    let saw_them = segments.iter().any(|s| s.channel == Channel::Them);
    match saw_you as u32 + saw_them as u32 {
        0 => None,
        count => Some(count),
    }
}

/// A stable `^a_[0-9a-z]{6,}$` id for an action item: an `a_` prefix over the
/// base36 FNV-1a-64 hash of the note id and the line's content.
///
/// FNV-1a is used deliberately over `std::hash::DefaultHasher` (SipHash), whose
/// output is not guaranteed stable across Rust releases — an action id must
/// survive a toolchain bump so a reindex never churns it. Inputs are joined with
/// a `\x1f` unit separator (which cannot appear in note text) so distinct field
/// splits never collide, and `occurrence` distinguishes two identical lines in
/// the same note. The note id in the input keys the same sentence in two
/// meetings to different ids.
fn action_item_id(
    note_id: &str,
    owner: &str,
    description: &str,
    due_date: Option<&str>,
    occurrence: u32,
) -> String {
    let mut hash = FNV_OFFSET;
    for part in [
        note_id,
        owner,
        description,
        due_date.unwrap_or(""),
        &occurrence.to_string(),
    ] {
        for &byte in part.as_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        hash ^= u64::from(UNIT_SEPARATOR);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("a_{}", to_base36_min6(hash))
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const UNIT_SEPARATOR: u8 = 0x1f;

/// Renders `n` as lowercase base36, left-padded with `0` to at least six
/// characters so the id always satisfies the schema's `{6,}` length floor.
fn to_base36_min6(mut n: u64) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = Vec::new();
    loop {
        digits.push(ALPHABET[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    while digits.len() < 6 {
        digits.push(b'0');
    }
    digits.reverse();
    // Every byte is an ASCII base36 digit, so this is valid UTF-8.
    String::from_utf8(digits).expect("base36 digits are ASCII")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn segment(index: u64, channel: Channel, start_ms: u64, end_ms: u64) -> TranscriptSegment {
        TranscriptSegment {
            index,
            channel,
            speaker: None,
            start_ms,
            end_ms,
            text: "hello".to_string(),
        }
    }

    /// Writes segments as JSONL exactly as `read_raw_session` expects, returning
    /// the repo-relative path to store as a note `source`.
    fn write_session(kb_root: &Path, segments: &[TranscriptSegment]) -> String {
        write_session_at(kb_root, "sessions/capture.jsonl", segments)
    }

    /// The same, at a caller-chosen path — so a test can put *real, readable*
    /// transcript bytes behind a `chats/` source and prove the type gate, not the
    /// absence of a parseable file.
    fn write_session_at(kb_root: &Path, rel: &str, segments: &[TranscriptSegment]) -> String {
        let path = kb_root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut contents = String::new();
        for segment in segments {
            contents.push_str(&serde_json::to_string(segment).unwrap());
            contents.push('\n');
        }
        std::fs::write(&path, contents).unwrap();
        rel.to_string()
    }

    const NOTE_ID: &str = "n_a1b2c3";

    #[test]
    fn action_lines_parse_owner_done_and_due() {
        let body = "\
# Summary

The team met.

## Action items

- [ ] Jane to send the memo by 2026-07-15.
- [x] Priya to book the room.
- [ ] Unassigned to circulate the notes.";
        let (_, items) = parse_body(NOTE_ID, "2026-07-09", body);

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].owner, "Jane");
        assert_eq!(items[0].description, "send the memo");
        assert_eq!(items[0].due_date.as_deref(), Some("2026-07-15"));
        assert!(!items[0].done);
        assert_eq!(items[0].extracted_date.as_deref(), Some("2026-07-09"));

        assert_eq!(items[1].owner, "Priya");
        assert!(items[1].done);
        assert_eq!(items[1].due_date, None);

        assert_eq!(items[2].owner, UNASSIGNED_OWNER);
    }

    #[test]
    fn decisions_are_collected_only_from_their_section() {
        let body = "\
# Summary

Body text.

## Decisions

- Ship on Friday
- Freeze the API

## Action items

- [ ] Jane to cut the release.

## Open questions

- What about docs?";
        let (decisions, items) = parse_body(NOTE_ID, "2026-07-09", body);

        assert_eq!(decisions, vec!["Ship on Friday", "Freeze the API"]);
        // The action item and the open question do not leak into decisions.
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn duplicate_identical_lines_get_distinct_but_stable_ids() {
        let body = "\
## Action items

- [ ] Jane to send the memo.
- [ ] Jane to send the memo.";
        let (_, first) = parse_body(NOTE_ID, "2026-07-09", body);
        let (_, second) = parse_body(NOTE_ID, "2026-07-09", body);

        assert_eq!(first.len(), 2);
        // Distinct within the note...
        assert_ne!(first[0].id, first[1].id);
        // ...and stable across a re-derivation.
        assert_eq!(first[0].id, second[0].id);
        assert_eq!(first[1].id, second[1].id);
    }

    #[test]
    fn ids_are_well_formed_and_scoped_by_note() {
        let body = "## Action items\n\n- [ ] Jane to send the memo by 2026-07-15.";
        let (_, a) = parse_body("n_aaaaaa", "2026-07-09", body);
        let (_, b) = parse_body("n_bbbbbb", "2026-07-09", body);

        let id = &a[0].id;
        let suffix = id.strip_prefix("a_").expect("a_ prefix");
        assert!(suffix.len() >= 6, "id {id} must have a 6+ char suffix");
        assert!(
            suffix
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
            "id {id} must be base36"
        );
        // The same line in two notes mints two different ids.
        assert_ne!(a[0].id, b[0].id);
    }

    #[test]
    fn a_description_containing_to_and_by_is_not_misparsed() {
        let body = "## Action items\n\n- [ ] Priya to talk to finance by phone by 2026-08-01.";
        let (_, items) = parse_body(NOTE_ID, "2026-07-09", body);

        assert_eq!(items[0].owner, "Priya");
        assert_eq!(items[0].description, "talk to finance by phone");
        assert_eq!(items[0].due_date.as_deref(), Some("2026-08-01"));
    }

    #[test]
    fn duration_and_speaker_count_come_from_the_transcript() {
        let kb = tempdir().unwrap();
        let rel = write_session(
            kb.path(),
            &[
                segment(0, Channel::You, 0, 4_000),
                segment(1, Channel::Them, 4_000, 12_500),
                segment(2, Channel::You, 12_500, 30_250),
            ],
        );
        let source = Source::parse(&rel).unwrap();

        let facts = derive_meeting_facts(
            NOTE_ID,
            NoteType::Meeting,
            "2026-07-09",
            &source,
            "",
            kb.path(),
        );
        assert_eq!(facts.duration_seconds, Some(30)); // 30_250 ms floors to 30 s
        assert_eq!(facts.speaker_count, Some(2)); // You + Them
    }

    #[test]
    fn a_single_channel_transcript_counts_one_speaker() {
        let kb = tempdir().unwrap();
        let rel = write_session(kb.path(), &[segment(0, Channel::You, 0, 5_000)]);
        let source = Source::parse(&rel).unwrap();

        let facts = derive_meeting_facts(
            NOTE_ID,
            NoteType::Meeting,
            "2026-07-09",
            &source,
            "",
            kb.path(),
        );
        assert_eq!(facts.speaker_count, Some(1));
    }

    #[test]
    fn an_unknown_only_transcript_has_no_resolvable_speaker_count() {
        let kb = tempdir().unwrap();
        let rel = write_session(kb.path(), &[segment(0, Channel::Unknown, 0, 5_000)]);
        let source = Source::parse(&rel).unwrap();

        let facts = derive_meeting_facts(
            NOTE_ID,
            NoteType::Meeting,
            "2026-07-09",
            &source,
            "",
            kb.path(),
        );
        assert_eq!(facts.duration_seconds, Some(5));
        assert_eq!(facts.speaker_count, None);
    }

    #[test]
    fn a_keyword_source_has_null_duration_and_speaker_count() {
        let kb = tempdir().unwrap();
        let source = Source::parse("manual").unwrap();

        let facts = derive_meeting_facts(
            NOTE_ID,
            NoteType::Meeting,
            "2026-07-09",
            &source,
            "",
            kb.path(),
        );
        assert_eq!(facts.duration_seconds, None);
        assert_eq!(facts.speaker_count, None);
    }

    #[test]
    fn a_pruned_session_file_yields_null_metrics() {
        let kb = tempdir().unwrap();
        // A path that was retention-pruned: the source points at a file that no
        // longer exists.
        let source = Source::parse("sessions/gone.jsonl").unwrap();

        let facts = derive_meeting_facts(
            NOTE_ID,
            NoteType::Meeting,
            "2026-07-09",
            &source,
            "",
            kb.path(),
        );
        assert_eq!(facts.duration_seconds, None);
        assert_eq!(facts.speaker_count, None);
    }

    /// Builds a note of the given type with one action item in its body.
    fn note_of(id: &str, note_type: NoteType, source: &str) -> Note {
        use crate::note::{NoteId, Routing, Tag};

        Note::new(
            NoteId::parse(id).unwrap(),
            note_type,
            Routing::Routed {
                project: "Ops".to_string(),
                confidence: 0.9,
            },
            "2026-07-09",
            Vec::<Tag>::new(),
            Source::parse(source).unwrap(),
            "## Decisions\n\n- Ship it.\n\n## Action items\n\n- [ ] Jane to do the thing.",
        )
        .unwrap()
    }

    #[test]
    fn facts_are_derived_for_the_types_whose_bodies_the_distill_pass_renders() {
        assert!(derives_facts(NoteType::Meeting));
        assert!(derives_facts(NoteType::Chat));
        // Deliberate: a `note` body is written verbatim by quick capture and
        // never passes through `distill::render_body`, so nothing guarantees it
        // follows the grammar `parse_action_line` inverts.
        assert!(!derives_facts(NoteType::Note));
    }

    #[test]
    fn facts_are_not_derived_for_a_plain_note() {
        let kb = PathBuf::from(".");
        let note = note_of("n_note01", NoteType::Note, "quick-capture");

        assert_eq!(meeting_facts_for(&note, &kb), None);
    }

    #[test]
    fn facts_are_derived_for_a_chat_note() {
        let kb = tempdir().unwrap();
        let note = note_of("n_chat01", NoteType::Chat, "chats/session.jsonl");

        let facts = meeting_facts_for(&note, kb.path()).expect("a chat note carries facts");
        assert_eq!(facts.decisions, vec!["Ship it.".to_string()]);
        assert_eq!(facts.action_items.len(), 1);
        assert_eq!(facts.action_items[0].owner, "Jane");
        assert_eq!(facts.action_items[0].description, "do the thing");
        // The two session scalars are the real meeting/chat divergence.
        assert_eq!(facts.duration_seconds, None);
        assert_eq!(facts.speaker_count, None);
    }

    #[test]
    fn a_chat_note_never_reads_its_source_as_a_transcript() {
        let kb = tempdir().unwrap();
        // Real, readable transcript JSONL — but behind a `chats/` source. If the
        // type gate were dropped, `session_metrics` would parse this happily and
        // report `Some(30)` / `Some(1)`, so this fails loudly rather than
        // silently agreeing with an unparseable file.
        let rel = write_session_at(
            kb.path(),
            "chats/session.jsonl",
            &[segment(0, Channel::You, 0, 30_000)],
        );
        let source = Source::parse(&rel).unwrap();

        let facts = derive_meeting_facts(
            "n_chat01",
            NoteType::Chat,
            "2026-07-09",
            &source,
            "",
            kb.path(),
        );
        assert_eq!(facts.duration_seconds, None);
        assert_eq!(facts.speaker_count, None);

        // Same bytes, same path, typed as a meeting: the read does happen. This
        // is what proves the assertions above are the gate and not the fixture.
        let as_meeting = derive_meeting_facts(
            "n_mtg01",
            NoteType::Meeting,
            "2026-07-09",
            &source,
            "",
            kb.path(),
        );
        assert_eq!(as_meeting.duration_seconds, Some(30));
        assert_eq!(as_meeting.speaker_count, Some(1));
    }
}
