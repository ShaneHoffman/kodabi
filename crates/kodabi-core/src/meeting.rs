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
//! **Scope: every note type** ([`derives_facts`]) — "chats are documents too"
//! (FOUNDING_DOC §3.6), and a commitment written by hand is as real as one a
//! meeting produced. The `meeting` names here (this module, [`MeetingFacts`],
//! the `note_meetings` table, `IndexedNote::meeting`) are historical: they
//! predate both the chat leg and the hand-written one, and were kept because the
//! MCP wire object is still `meeting`/`MeetingMeta`, which stays meeting-only.
//!
//! **Two grammars, chosen by type** ([`parse_body`]). A machine-rendered body
//! (meeting, chat) round-trips through the distill grammar by construction —
//! [`parse_action_line`] is documented as `render_body`'s exact inverse. A
//! hand-written body (`note`) makes no such promise, so it gets the
//! plain-checkbox grammar instead, which infers nothing from the line's text.
//!
//! The two session-derived scalars are the other divergence — they are `None`
//! for anything but a meeting, whose `source` alone points at a session
//! recording.
//!
//! Everything here is a pure function of the note's body + its session file, so a
//! reindex reproduces it exactly — the index stays a rebuildable cache
//! (FOUNDING_DOC §3.6). In particular an action item's id is a deterministic hash
//! of its content (below), never a random mint, so it is stable across reindexes.

use std::collections::HashMap;
use std::path::Path;

use crate::distill::{parse_action_line, parse_checkbox_line, SELF_OWNER, UNASSIGNED_OWNER};
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
    /// Whether the line reads as an actual commitment rather than a tentative
    /// one — the absence of `distill`'s soft marker. The ledger's enrollment
    /// gate refuses to track a soft item; nothing else treats it differently,
    /// and it is deliberately not an [`action_item_id`] input, so editing the
    /// marker by hand promotes or demotes the item without re-minting its id.
    ///
    /// A plain note's checkbox is always firm: that grammar has no marker, and
    /// a line the user wrote by hand is their own commitment by construction.
    pub firm: bool,
    /// The meeting's own calendar day (from the note `date`), or `None` when the
    /// date is unparseable.
    pub extracted_date: Option<String>,
}

/// Whether a note of this type carries facts worth deriving — the single source
/// of truth for the gate, mirrored in SQL by
/// [`note_ids_missing_meeting_facts`](crate::index::NoteIndex::note_ids_missing_meeting_facts).
///
/// Every note type carries facts, but not by the same grammar — see
/// [`parse_body`]. A machine-rendered body (meeting, chat) is read with the
/// distill grammar; a hand-written one (`note`) is read with the plain-checkbox
/// grammar.
///
/// This returns `true` for every variant today, and is kept as a function rather
/// than inlined because it is a *contract*, not a constant: it is the single
/// source of truth a new note type must answer, and the SQL mirror above is
/// pinned to it by a parity test.
///
/// [`NoteType::Note`] was excluded until the commitment ledger shipped, on the
/// grounds that `quick_capture` ([`crate::capture`]) writes the user's text
/// verbatim so nothing guarantees a `note` body follows the distill grammar —
/// parsed by that grammar, a hand-written `- [ ] Send the deck to Priya.` would
/// take everything before the first `" to "` as the owner and index with owner
/// `"Send the deck"`. That argument is answered rather than overridden: a plain
/// note is never read with the distill grammar at all, so the misparse it warned
/// about cannot arise.
pub fn derives_facts(note_type: NoteType) -> bool {
    matches!(
        note_type,
        NoteType::Meeting | NoteType::Chat | NoteType::Note
    )
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
    let (decisions, action_items) = parse_body(note_id, note_type, date, body);
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

/// Extracts a note's decisions and action items, by whichever of the two
/// grammars its type is written in.
///
/// **Machine-rendered bodies** ([`NoteType::Meeting`], [`NoteType::Chat`]) use
/// the distill grammar: a `##` header switches the active section; any other
/// `#`-header (e.g. `# Summary`) resets to `Other`, so only lines under
/// `## Decisions` and `## Action items` are collected, and each action line is
/// split into owner / description / due date by [`parse_action_line`].
/// Unparseable action lines are skipped.
///
/// **Hand-written bodies** ([`NoteType::Note`]) use the plain-checkbox grammar,
/// and the two differences are both deliberate:
///
/// - *No sections.* Any `- [ ]` / `- [x]` line counts, wherever it sits. Quick
///   capture writes the user's text with no headers at all, so requiring
///   `## Action items` would exclude exactly the notes this grammar exists for.
/// - *No owner split.* The whole line after the checkbox is the description and
///   the owner is always [`SELF_OWNER`]. A plain note is the user's own
///   scratchpad, and applying the distill grammar's `" to "` split to free text
///   is the misparse [`derives_facts`] documents. Nothing else is inferred: no
///   due date is parsed out of prose either.
///
/// A plain note has no decisions — `## Decisions` is a rendered-body construct,
/// and inferring decisions from free text is not something a grammar can do.
fn parse_body(
    note_id: &str,
    note_type: NoteType,
    date: &str,
    body: &str,
) -> (Vec<String>, Vec<ActionItemFact>) {
    let extracted_date = date.get(..10).and_then(crate::distill::valid_iso_date);

    let mut decisions = Vec::new();
    let mut action_items = Vec::new();
    // Occurrences of each identical (owner, description, due) tuple seen so far,
    // so duplicate lines get distinct-but-stable ids.
    let mut occurrences: HashMap<(String, String, Option<String>), u32> = HashMap::new();

    if note_type == NoteType::Note {
        for raw_line in body.lines() {
            let Some((rest, done)) = parse_checkbox_line(raw_line.trim()) else {
                continue;
            };
            let description = rest.trim();
            if description.is_empty() {
                continue;
            }
            let description = description.to_string();
            let key = (SELF_OWNER.to_string(), description.clone(), None);
            let occurrence = occurrences.entry(key).or_insert(0);
            let id = action_item_id(note_id, SELF_OWNER, &description, None, *occurrence);
            *occurrence += 1;
            action_items.push(ActionItemFact {
                id,
                description,
                owner: SELF_OWNER.to_string(),
                due_date: None,
                done,
                firm: true,
                extracted_date: extracted_date.clone(),
            });
        }
        return (decisions, action_items);
    }

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
                if let Some((owner, description, due_date, firm)) = parse_action_line(line) {
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
                        firm,
                        extracted_date: extracted_date.clone(),
                    });
                }
            }
            Section::Other => {}
        }
    }

    (decisions, action_items)
}

/// The 0-based body line index of the action item with `item_id`, or `None` when
/// no line in this body mints that id.
///
/// Walks the body exactly as [`parse_body`] does — **same grammar for the same
/// `note_type`**, same section state machine, same occurrence counting — because
/// the occurrence counter is what distinguishes two identical lines, so a walk
/// that diverged from it would return the wrong line for the second of a
/// duplicate pair. The two mode branches here mirror `parse_body`'s and must be
/// kept in lockstep with them.
///
/// `None` is an ordinary answer, not a failure: an item whose line has since
/// been edited or deleted no longer exists in the body, and the caller
/// (`vault::annotate_action_item`) treats annotating as best-effort.
pub fn action_item_line(
    note_id: &str,
    note_type: NoteType,
    body: &str,
    item_id: &str,
) -> Option<usize> {
    let mut occurrences: HashMap<(String, String, Option<String>), u32> = HashMap::new();

    if note_type == NoteType::Note {
        for (index, raw_line) in body.lines().enumerate() {
            let Some((rest, _done)) = parse_checkbox_line(raw_line.trim()) else {
                continue;
            };
            let description = rest.trim();
            if description.is_empty() {
                continue;
            }
            let key = (SELF_OWNER.to_string(), description.to_string(), None);
            let occurrence = occurrences.entry(key).or_insert(0);
            let id = action_item_id(note_id, SELF_OWNER, description, None, *occurrence);
            *occurrence += 1;
            if id == item_id {
                return Some(index);
            }
        }
        return None;
    }

    let mut section = Section::Other;

    for (index, raw_line) in body.lines().enumerate() {
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
        if !matches!(section, Section::ActionItems) {
            continue;
        }
        // Firmness is not an id input, so `action_item_line` ignores it: a line
        // whose only edit was the soft marker still resolves to the same item.
        let Some((owner, description, due_date, _firm)) = parse_action_line(line) else {
            continue;
        };
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
        if id == item_id {
            return Some(index);
        }
    }
    None
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

    /// The distill-grammar parse. Every test that predates the plain-note
    /// grammar exercises a machine-rendered body, so they name the type once
    /// here rather than repeating it at each call.
    fn parse_meeting(note_id: &str, date: &str, body: &str) -> (Vec<String>, Vec<ActionItemFact>) {
        parse_body(note_id, NoteType::Meeting, date, body)
    }

    /// The distill-grammar reverse walk, paired with [`parse_meeting`].
    fn meeting_item_line(note_id: &str, body: &str, item_id: &str) -> Option<usize> {
        action_item_line(note_id, NoteType::Meeting, body, item_id)
    }

    // --- action_item_line -------------------------------------------------

    const ANNOTATED_BODY: &str = "# Summary\n\nWe met.\n\n## Action items\n\n\
         - [ ] Priya to send the deck by 2026-08-20.\n\
         - [x] You to book the venue.\n\
         - [ ] Priya to send the deck by 2026-08-20.\n";

    #[test]
    fn action_item_line_finds_each_item_including_duplicates() {
        let items = parse_meeting("n_aaaaaa", "2026-08-01", ANNOTATED_BODY).1;
        assert_eq!(items.len(), 3);

        // Body line indexes: 0 `# Summary`, 1 blank, 2 prose, 3 blank,
        // 4 `## Action items`, 5 blank, then the three items.
        assert_eq!(
            meeting_item_line("n_aaaaaa", ANNOTATED_BODY, &items[0].id),
            Some(6)
        );
        assert_eq!(
            meeting_item_line("n_aaaaaa", ANNOTATED_BODY, &items[1].id),
            Some(7)
        );
        // The duplicate resolves to the *second* occurrence's line, which only
        // the shared occurrence counting can get right.
        assert_eq!(
            meeting_item_line("n_aaaaaa", ANNOTATED_BODY, &items[2].id),
            Some(8)
        );
    }

    #[test]
    fn action_item_line_is_none_for_an_unknown_or_foreign_id() {
        let items = parse_meeting("n_aaaaaa", "2026-08-01", ANNOTATED_BODY).1;
        assert_eq!(
            meeting_item_line("n_aaaaaa", ANNOTATED_BODY, "a_notreal"),
            None
        );
        // Ids are scoped by note, so another note's id never matches here.
        assert_eq!(
            meeting_item_line("n_bbbbbb", ANNOTATED_BODY, &items[0].id),
            None
        );
    }

    #[test]
    fn a_closure_annotation_is_inert_to_the_grammar() {
        // The line `vault::annotate_action_item` writes must parse as nothing:
        // no phantom item, and every real id unchanged.
        let plain = "## Action items\n\n- [ ] Priya to send the deck.\n";
        let annotated = "## Action items\n\n- [ ] Priya to send the deck.\n  \
             - Closed 2026-08-17: PR merged (example.com/pull/42).\n";

        let before = parse_meeting("n_aaaaaa", "2026-08-01", plain).1;
        let after = parse_meeting("n_aaaaaa", "2026-08-01", annotated).1;
        assert_eq!(before, after);
        assert_eq!(after.len(), 1, "the annotation minted no item");
    }

    // --- the plain-note grammar -------------------------------------------

    #[test]
    fn a_plain_note_takes_checkboxes_anywhere_as_the_users_own() {
        // A quick capture: no frontmatter sections, no headers at all.
        let body = "Called the bank, still waiting.\n\
             - [ ] chase the wire\n\
             \n\
             Some other thought.\n\
             - [x] book the flights\n";
        let (decisions, items) = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", body);

        assert!(decisions.is_empty(), "a plain note renders no decisions");
        assert_eq!(items.len(), 2, "both lines count, section or not");

        assert_eq!(items[0].owner, SELF_OWNER);
        assert_eq!(items[0].description, "chase the wire");
        assert_eq!(items[0].due_date, None);
        assert!(!items[0].done);
        assert_eq!(items[0].extracted_date.as_deref(), Some("2026-07-09"));

        assert_eq!(items[1].owner, SELF_OWNER);
        assert_eq!(items[1].description, "book the flights");
        assert!(items[1].done);
    }

    #[test]
    fn a_plain_note_line_never_splits_on_to() {
        // The exact misparse `derives_facts` documented as the reason plain
        // notes were excluded: under the distill grammar this line indexes with
        // owner "Send the deck" and description "Priya".
        let body = "- [ ] Send the deck to Priya.";

        let plain = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", body).1;
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].owner, SELF_OWNER);
        assert_eq!(plain[0].description, "Send the deck to Priya.");
        assert_eq!(plain[0].due_date, None);

        // The same body read as a meeting still misparses, which is why the two
        // grammars are separate rather than one widened one.
        let as_meeting = parse_meeting(NOTE_ID, "2026-07-09", &format!("## Action items\n{body}"));
        assert_eq!(as_meeting.1[0].owner, "Send the deck");
    }

    #[test]
    fn a_plain_note_ignores_prose_and_bare_bullets() {
        let body = "# A heading is just text here\n\
             - a plain bullet\n\
             - [] malformed\n\
             -[ ] malformed\n\
             - [X] uppercase is not the marker\n\
             - [ ]\n\
             - [ ]    \n";
        let items = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", body).1;
        assert!(
            items.is_empty(),
            "only well-formed, non-empty checkbox lines count, got {items:?}"
        );
    }

    #[test]
    fn a_closure_annotation_is_inert_in_a_plain_note() {
        // `vault::annotate_action_item` writes its line into whatever note the
        // item lives in, so the plain grammar has to skip it too. It does by
        // construction: `- Closed ` is neither checkbox marker.
        let plain = "- [ ] chase the wire\n";
        let annotated = "- [ ] chase the wire\n  - Closed 2026-08-17: paid.\n";

        let before = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", plain).1;
        let after = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", annotated).1;
        assert_eq!(before, after, "ids and all");
        assert_eq!(after.len(), 1, "the annotation minted no item");
    }

    /// Firmness must be invisible to the id, or promoting a tentative item by
    /// deleting its marker would re-mint the id and orphan whatever the ledger
    /// had already linked to it. The reverse walk has to agree, since that is
    /// how a line gets rewritten in place.
    #[test]
    fn the_soft_marker_changes_firmness_and_nothing_else() {
        let firm = "# Summary\n\nWe met.\n\n## Action items\n\n\
             - [ ] Priya to look into pricing.\n";
        let soft = "# Summary\n\nWe met.\n\n## Action items\n\n\
             - [ ] Priya to look into pricing. (tentative)\n";

        let firm_items = parse_meeting("n_aaaaaa", "2026-08-01", firm).1;
        let soft_items = parse_meeting("n_aaaaaa", "2026-08-01", soft).1;

        assert_eq!(firm_items.len(), 1);
        assert_eq!(soft_items.len(), 1);
        assert!(firm_items[0].firm);
        assert!(!soft_items[0].firm);
        assert_eq!(
            firm_items[0].id, soft_items[0].id,
            "the id ignores firmness"
        );
        assert_eq!(firm_items[0].description, soft_items[0].description);
        assert_eq!(firm_items[0].owner, soft_items[0].owner);

        assert_eq!(
            meeting_item_line("n_aaaaaa", soft, &soft_items[0].id),
            Some(6),
            "the reverse walk finds a soft line by the same id"
        );
    }

    /// The hand-written grammar has no marker, so a line a person typed is
    /// their own commitment by construction.
    #[test]
    fn a_plain_note_checkbox_is_always_firm() {
        let body = "- [ ] chase the wire\n- [ ] chase the wire (tentative)\n";
        let items = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", body).1;

        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.firm));
        assert_eq!(items[1].description, "chase the wire (tentative)");
    }

    #[test]
    fn plain_note_duplicates_get_distinct_but_stable_ids() {
        let body = "- [ ] chase the wire\n- [ ] chase the wire\n";
        let first = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", body).1;
        let second = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", body).1;

        assert_eq!(first.len(), 2);
        assert_ne!(first[0].id, first[1].id);
        assert_eq!(first, second, "stable across a re-derivation");
    }

    #[test]
    fn action_item_line_walks_the_plain_grammar() {
        let body = "Notes.\n\
             - [ ] chase the wire\n\
             prose in between\n\
             - [ ] chase the wire\n";
        let items = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", body).1;
        assert_eq!(items.len(), 2);

        assert_eq!(
            action_item_line(NOTE_ID, NoteType::Note, body, &items[0].id),
            Some(1)
        );
        // The duplicate resolves to its own line, which only the shared
        // occurrence counting gets right.
        assert_eq!(
            action_item_line(NOTE_ID, NoteType::Note, body, &items[1].id),
            Some(3)
        );
        assert_eq!(
            action_item_line(NOTE_ID, NoteType::Note, body, "a_notreal"),
            None
        );
    }

    #[test]
    fn the_two_grammars_do_not_read_each_others_bodies() {
        // A meeting body's items are invisible to the plain grammar unless they
        // happen to be checkbox lines (they are, but they parse whole), and a
        // plain body's items are invisible to the meeting grammar (no section).
        let meeting_body = "## Action items\n\n- [ ] Jane to send the memo.";
        let plain_read = parse_body(NOTE_ID, NoteType::Note, "2026-07-09", meeting_body).1;
        assert_eq!(plain_read.len(), 1);
        assert_eq!(plain_read[0].owner, SELF_OWNER);
        assert_eq!(plain_read[0].description, "Jane to send the memo.");

        let plain_body = "- [ ] chase the wire";
        let meeting_read = parse_meeting(NOTE_ID, "2026-07-09", plain_body).1;
        assert!(meeting_read.is_empty(), "no section, no items");
    }

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
        let (_, items) = parse_meeting(NOTE_ID, "2026-07-09", body);

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
        let (decisions, items) = parse_meeting(NOTE_ID, "2026-07-09", body);

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
        let (_, first) = parse_meeting(NOTE_ID, "2026-07-09", body);
        let (_, second) = parse_meeting(NOTE_ID, "2026-07-09", body);

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
        let (_, a) = parse_meeting("n_aaaaaa", "2026-07-09", body);
        let (_, b) = parse_meeting("n_bbbbbb", "2026-07-09", body);

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
        let (_, items) = parse_meeting(NOTE_ID, "2026-07-09", body);

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
    fn facts_are_derived_for_every_note_type() {
        // A `note` body joined the list when the commitment ledger widened to
        // hand-written commitments. It is not read with the distill grammar —
        // it gets the plain-checkbox one — so the misparse that kept it out is
        // answered rather than accepted.
        for note_type in [NoteType::Meeting, NoteType::Chat, NoteType::Note] {
            assert!(derives_facts(note_type), "{note_type:?} must derive facts");
        }
    }

    #[test]
    fn a_plain_notes_facts_come_from_the_plain_grammar() {
        let kb = PathBuf::from(".");
        let note = note_of("n_note01", NoteType::Note, "quick-capture");

        let facts = meeting_facts_for(&note, &kb).expect("a plain note carries facts");
        // The shared fixture body is meeting-shaped, which is the point: read
        // with the plain grammar its headers are inert, its checkbox line is
        // the user's own, and its `## Decisions` bullet is not a decision.
        assert!(facts.decisions.is_empty());
        assert_eq!(facts.action_items.len(), 1);
        assert_eq!(facts.action_items[0].owner, SELF_OWNER);
        assert_eq!(
            facts.action_items[0].description, "Jane to do the thing.",
            "no owner split, no terminal period peeled"
        );
        // Session scalars are meeting-only, as for a chat.
        assert_eq!(facts.duration_seconds, None);
        assert_eq!(facts.speaker_count, None);
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
