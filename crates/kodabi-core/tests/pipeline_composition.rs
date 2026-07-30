//! The distill pipeline as a *chain*: a captured artifact is distilled, routed
//! into a real vault, written as Markdown, indexed, and searched back. Once for
//! a meeting transcript (`sessions/*.jsonl` → `type: meeting`) and once for a
//! chat transcript (`chats/*.jsonl` → `type: chat`).
//!
//! Every stage here already has thorough inline coverage. What none of it can
//! see is the seams *between* the stages — a `source:` form routing accepts but
//! [`vault::collect_raw_artifact_sources`] doesn't, a note the writer produces
//! but the indexer skips, a `date` the writer emits but
//! [`normalize_date_to_utc`] mis-orders. Each stage passes its own tests and the
//! chain still breaks. These tests exercise the composition, so they assert on
//! what one stage hands the next rather than re-testing what either does
//! internally.
//!
//! The two legs share every stage from the model call down (`distill_rendered`),
//! so what the chat leg is really for is where they *diverge*: a second raw
//! store with its own claim list ([`chats::list_undistilled_chats`], and
//! deliberately never [`sessions::list_failed_sessions`] — a chat transcript
//! must not read as an unclaimed capture), a `date` recovered from the
//! transcript's own `Meta` record rather than from its filename, and a note type
//! [`meeting::meeting_facts_for`] declines to derive facts for, so a chat's
//! rendered action items never reach the index tables.
//!
//! Deliberately a scripted [`MockRunner`], never a real `claude`: the subject is
//! composition, not model quality. That is what keeps this file out of the
//! `#[ignore]`d tier — it runs under plain `cargo test --workspace` on every
//! commit, which is the only way a composition regression gets caught at the
//! commit that causes it.
//!
//! ```text
//! cargo test -p kodabi-core --test pipeline_composition
//! ```

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, TimeZone, Timelike, Utc};
use serde_json::json;
use tempfile::{tempdir, TempDir};

use kodabi_core::chat::{ChatRecord, ChatTranscript, PermissionResolution};
use kodabi_core::chat_distill;
use kodabi_core::chats;
use kodabi_core::device::DeviceId;
use kodabi_core::distill::{self, DistillOutput};
use kodabi_core::embed::{index_note, FakeEmbedder};
use kodabi_core::glossary::{Glossary, GlossaryTerm, OnConflict};
use kodabi_core::index::{
    normalize_date_to_utc, IndexedNote, NoteIndex, NoteType as IndexedNoteType, ProjectScope,
    SearchParams, SearchResults, TagMatch,
};
use kodabi_core::llm::{HeadlessClaude, LlmRequest, LlmRunError};
use kodabi_core::meeting;
use kodabi_core::note::{Note, NoteType, Routing, INBOX};
use kodabi_core::raw_session::{self, TranscriptSegment};
use kodabi_core::routing::{RoutingConfig, DEFAULT_THRESHOLD};
use kodabi_core::routing_examples::{RoutingExample, RoutingExamples};
use kodabi_core::sessions;
use kodabi_core::transcription::Channel;
use kodabi_core::vault;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn segment(index: u64, channel: Channel, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        index,
        channel,
        speaker: None,
        start_ms: index * 1000,
        end_ms: index * 1000 + 500,
        text: text.to_owned(),
    }
}

fn device() -> DeviceId {
    DeviceId::parse("k4m2xp7q").unwrap()
}

/// A capture instant on the given July 2026 day, carrying sub-second precision
/// so the date the writer emits genuinely exercises its millisecond format.
fn instant_on(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, day, 14, 3, 35)
        .unwrap()
        .with_nanosecond(123_000_000)
        .unwrap()
}

/// Writes a transcript into `<vault>/sessions/` through the production writer,
/// so the bytes on disk are exactly what a real capture leaves behind.
fn write_session(
    vault: &Path,
    at: DateTime<Utc>,
    slug: &str,
    segments: &[TranscriptSegment],
) -> PathBuf {
    raw_session::write_raw_session(&vault.join("sessions"), at, &device(), Some(slug), segments)
        .unwrap()
}

/// Writes a chat transcript into `<vault>/chats/` through the production
/// writer, so the filename scheme and the JSONL bytes are exactly what a live
/// session leaves behind.
///
/// Note the asymmetry with [`write_session`]: `ChatTranscript::create` takes the
/// *knowledge-base root* and joins `chats/` itself, so passing
/// `vault.join("chats")` here would nest a second one and quietly break every
/// claim assertion below.
fn write_chat(vault: &Path, at: DateTime<Utc>, records: &[ChatRecord]) -> PathBuf {
    let transcript = ChatTranscript::create(vault, &device(), at).expect("creating the transcript");
    for record in records {
        transcript.append(record).expect("appending a chat record");
    }
    transcript.path().to_path_buf()
}

/// Replays one canned distill response for every call.
struct MockRunner(String);

impl HeadlessClaude for MockRunner {
    fn run(&self, _request: &LlmRequest) -> Result<String, LlmRunError> {
        Ok(self.0.clone())
    }
}

/// `note::project_dir` is `pub(crate)`, so an integration test rebuilds the
/// slug → folder mapping the same way: split the hierarchical slug on `/`.
fn project_dir(vault: &Path, project: &str) -> PathBuf {
    project
        .split('/')
        .fold(vault.to_path_buf(), |dir, segment| dir.join(segment))
}

fn save_glossary(vault: &Path, project: &str, terms: &[&str]) {
    let mut glossary = Glossary::default();
    for term in terms {
        glossary
            .upsert(
                GlossaryTerm {
                    term: (*term).to_string(),
                    definition: String::new(),
                    aliases: Vec::new(),
                },
                OnConflict::Error,
            )
            .unwrap();
    }
    let dir = project_dir(vault, project);
    std::fs::create_dir_all(&dir).unwrap();
    glossary.save(&dir).unwrap();
}

/// Seeds a project's recorded corrections. Not decorative: `route`'s scorer adds
/// `EXAMPLE_WEIGHT * similarity` per candidate, so a matching example is real
/// weight behind the routing decision the chain then persists.
fn save_routing_examples(vault: &Path, project: &str, examples: &[(&str, &str, &str)]) {
    let mut log = RoutingExamples::default();
    for (note_id, title, excerpt) in examples {
        log.upsert(RoutingExample {
            note_id: (*note_id).to_string(),
            title: (*title).to_string(),
            excerpt: (*excerpt).to_string(),
            previous_project: None,
            confidence: 1.0,
            corrected_at: "2026-07-01T00:00:00Z".to_string(),
            reason: None,
        });
    }
    let dir = project_dir(vault, project);
    std::fs::create_dir_all(&dir).unwrap();
    log.save(&dir).unwrap();
}

/// Two real projects, each carrying **both** signal files, so routing runs over
/// the same on-disk shape a real vault has. Mirrors the fixture tone in
/// `distill.rs`'s own routing tests.
fn two_project_vault() -> TempDir {
    let vault = tempdir().unwrap();
    let root = vault.path();

    save_glossary(
        root,
        "Briarwood Golf",
        &[
            "MERIDIAN",
            "TeeTrack",
            "GreenFlow",
            "irrigation",
            "tee sheet",
        ],
    );
    save_routing_examples(
        root,
        "Briarwood Golf",
        &[(
            "n_a1b2c3",
            "Irrigation vendor review",
            "Reviewed the MERIDIAN irrigation quote against the TeeTrack maintenance log.",
        )],
    );

    save_glossary(root, "Growth/Q3", &["OKR", "activation"]);
    save_routing_examples(
        root,
        "Growth/Q3",
        &[(
            "n_d4e5f6",
            "Activation OKR check-in",
            "Walked the activation dashboard and agreed the funnel targets for the quarter.",
        )],
    );

    vault
}

/// The real router, recording what it decided so a test can assert the score
/// routing computed is byte-for-byte the one that reaches frontmatter and the
/// index. Pinning the decision this way beats pinning weight arithmetic: the
/// claim under test is that the number survives the chain, not what it is.
fn recording_route<'a>(
    vault: &'a Path,
    decided: &'a RefCell<Option<Routing>>,
) -> impl Fn(&DistillOutput, &str) -> Routing + 'a {
    move |output, body| {
        let (routing, diagnostics) =
            distill::route_distilled(vault, output, body, &RoutingConfig::default());
        // The fixture's signals are well-formed, so any degradation here means
        // the fixture drifted — and a fail-soft router would hide that.
        assert!(
            diagnostics.discovery_failure.is_none(),
            "project discovery failed: {:?}",
            diagnostics.discovery_failure
        );
        assert!(
            diagnostics.glossary_failures.is_empty(),
            "glossary failed to load: {:?}",
            diagnostics.glossary_failures
        );
        assert!(
            diagnostics.example_failures.is_empty(),
            "routing examples failed to load: {:?}",
            diagnostics.example_failures
        );
        *decided.borrow_mut() = Some(routing.clone());
        routing
    }
}

/// The write → index composition, exactly as the production write path does it:
/// parse the note back off disk, derive the effective title and the KB-relative
/// path, then fill in the meeting facts `from_note` deliberately leaves `None`.
fn index_written_note(index: &mut NoteIndex, vault: &Path, note_path: &Path) -> Note {
    let markdown = std::fs::read_to_string(note_path).unwrap();
    let note = Note::from_markdown(&markdown).expect("the writer's output should parse back");
    let relative = note_path.strip_prefix(vault).unwrap();

    let mut indexed = IndexedNote::from_note(
        &note,
        &vault::effective_title(&note, note_path),
        &relative.to_string_lossy().replace('\\', "/"),
    );
    indexed.meeting = meeting::meeting_facts_for(&note, vault);

    index_note(index, &indexed, Some(&FakeEmbedder)).expect("indexing the written note");
    note
}

fn query(text: &str) -> SearchParams {
    SearchParams {
        query: text.to_string(),
        project: None,
        include_descendants: true,
        types: Vec::new(),
        tags: Vec::new(),
        tag_match: TagMatch::Any,
        date_from: None,
        date_to: None,
        limit: 10,
        cursor: None,
    }
}

fn search(index: &NoteIndex, text: &str) -> SearchResults {
    index
        .search_notes(&query(text), Some(&FakeEmbedder))
        .expect("search should succeed")
}

fn session_file_names(vault: &Path) -> Vec<String> {
    sessions::list_failed_sessions(vault)
        .expect("deriving the needs-attention list")
        .into_iter()
        .map(|session| session.file_name)
        .collect()
}

/// A settle horizon past every wall-clock instant this suite can run at. Not the
/// fixture's date: `list_undistilled_chats` compares the transcript's *mtime*,
/// which is "now", against this. The grace window is `chats.rs`'s own concern and
/// has its own tests; here it must simply not interfere. Same value those tests
/// use.
fn settled() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()
}

/// The `chats/` twin of [`session_file_names`]: the transcripts the startup
/// sweep would still pick up because no note claims them.
fn undistilled_chat_names(vault: &Path) -> Vec<String> {
    chats::list_undistilled_chats(vault, settled())
        .expect("deriving the undistilled-chat list")
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

/// Floats survive the chain through a YAML scalar and a SQLite REAL, so compare
/// with a tolerance far tighter than any rounding either would introduce.
fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{what}: expected {expected}, got {actual}"
    );
}

// ---------------------------------------------------------------------------
// the meeting chain
// ---------------------------------------------------------------------------

fn briarwood_segments() -> Vec<TranscriptSegment> {
    vec![
        segment(
            0,
            Channel::You,
            "how are we tracking on the irrigation rebuild",
        ),
        segment(
            1,
            Channel::Them,
            "MERIDIAN sent the revised quote this morning",
        ),
        segment(
            2,
            Channel::You,
            "and the TeeTrack export for the tee sheet?",
        ),
        segment(
            3,
            Channel::Them,
            "Jane is pulling it together before the fifteenth",
        ),
    ]
}

/// The distilled output for [`briarwood_segments`]. The summary carries a phrase
/// that appears **nowhere else** — not in the transcript, the title, or the tags
/// — so a later search that finds it can only have matched the indexed note
/// body.
fn briarwood_output() -> String {
    r#"{
        "title": "Irrigation rebuild kickoff",
        "summary": "MERIDIAN returned the revised irrigation quote. The back nine sprinklers run dry by August, so the rebuild has to land before the club championship.",
        "decisions": ["Approve the MERIDIAN irrigation rebuild quote"],
        "action_items": [
            {"owner": "Jane", "description": "send the TeeTrack tee sheet export", "due_date": "2026-08-15"},
            {"owner": null, "description": "book the follow-up walkthrough", "due_date": null}
        ],
        "open_questions": ["Who owns the tee sheet migration?"],
        "tags": ["irrigation", "phase-2"]
    }"#
    .to_string()
}

#[test]
fn a_meeting_transcript_distills_routes_writes_indexes_and_searches_back() {
    let vault = two_project_vault();
    let root = vault.path();
    let session = write_session(root, instant_on(12), "budget-sync", &briarwood_segments());
    let session_name = session.file_name().unwrap().to_str().unwrap().to_string();

    // Before the chain runs the capture is unclaimed, so it needs attention.
    // Without this the "no longer listed" assertion below would also pass on a
    // list that was empty for the wrong reason.
    assert_eq!(
        session_file_names(root),
        vec![session_name.clone()],
        "an undistilled capture should need attention"
    );

    let decided = RefCell::new(None);
    let distilled = distill::distill_session(
        &MockRunner(briarwood_output()),
        root,
        &session,
        &recording_route(root, &decided),
    )
    .expect("distilling the session");

    // --- routing → write -------------------------------------------------
    let routed = decided.borrow().clone().expect("the router ran");
    assert_eq!(routed.project(), "Briarwood Golf");
    let score = routed.confidence().expect("a routed note carries a score");
    assert!(
        score >= DEFAULT_THRESHOLD,
        "a glossary-rich meeting should clear the routing threshold, got {score}"
    );
    assert_eq!(
        distilled.path.parent(),
        Some(root.join("Briarwood Golf").as_path()),
        "the note should land in the project routing chose"
    );

    // --- write → frontmatter ---------------------------------------------
    // One index for the whole chain: the note is parsed back off disk and
    // indexed once, and every assertion from here down reads that same pass.
    let mut index = NoteIndex::open_in_memory().unwrap();
    let note = index_written_note(&mut index, root, &distilled.path);
    assert_eq!(note.note_type, NoteType::Meeting);
    assert_eq!(note.routing.project(), "Briarwood Golf");
    assert_close(
        note.routing.confidence().unwrap(),
        score,
        "the score routing computed should be the one on disk",
    );
    assert_eq!(
        note.source.as_yaml(),
        format!("sessions/{session_name}"),
        "the note must claim its session in the exact form the sessions list joins on"
    );
    assert!(
        note.body.contains("back nine sprinklers run dry"),
        "body: {}",
        note.body
    );

    // The writer emits `date` as RFC 3339 with millisecond precision and a
    // numeric offset (never `Z`); the index has to parse that exact shape to
    // derive its ordering key.
    let date_utc = normalize_date_to_utc(&note.date)
        .unwrap_or_else(|err| panic!("the writer's date {:?} must normalize: {err}", note.date));
    assert!(date_utc.ends_with('Z'), "ordering key: {date_utc}");

    // --- write → index → search ------------------------------------------
    let id = note.id.as_str();

    let row = index
        .get_note(id)
        .unwrap()
        .expect("the written note should be in the index");
    assert_eq!(row.note_type, IndexedNoteType::Meeting);
    assert_eq!(row.project.as_deref(), Some("Briarwood Golf"));
    assert_eq!(row.date_utc, date_utc);
    assert!(
        index.note_has_chunks(id).unwrap(),
        "the note body should have been chunked and embedded"
    );

    // Meeting facts are derived from the body the writer rendered plus the
    // session the note points at — the note↔source pairing, end to end.
    let facts = index
        .get_meeting_facts(id)
        .unwrap()
        .expect("a meeting note should carry facts");
    // Note the trailing period: `parse_output` normalizes each decision to a
    // complete sentence, `render_body` writes that into the Markdown, and the
    // facts derivation reads the bullet back verbatim. Three stages, one string.
    assert_eq!(
        facts.decisions,
        vec!["Approve the MERIDIAN irrigation rebuild quote."]
    );
    assert_eq!(facts.speaker_count, Some(2), "You + Them");
    assert_eq!(
        facts.duration_seconds,
        Some(3),
        "last segment ends at 3500ms"
    );
    let items = index.get_action_items(id).unwrap();
    assert_eq!(items.len(), 2, "action items: {items:?}");
    assert_eq!(items[0].owner, "Jane");
    assert_eq!(items[0].due_date.as_deref(), Some("2026-08-15"));

    // A phrase that only ever existed in the distilled body.
    let hits = search(&index, "back nine sprinklers run dry").hits;
    assert_eq!(hits.len(), 1, "hits: {hits:?}");
    assert_eq!(hits[0].id, id);
    assert_eq!(hits[0].project.as_deref(), Some("Briarwood Golf"));
    assert_close(
        hits[0].confidence.unwrap(),
        score,
        "the score should survive all the way into a search hit",
    );

    // --- the claim seam ---------------------------------------------------
    assert!(
        session_file_names(root).is_empty(),
        "a distilled session must stop needing attention"
    );
}

// ---------------------------------------------------------------------------
// the chat chain
// ---------------------------------------------------------------------------

/// A conversation with the record mix a live session actually leaves behind: a
/// `Meta` header, turns in both voices, one tool call, one denied permission,
/// one failed turn. `started_at` is written at seconds precision because that is
/// what `chat_cmds` writes, so a chat note's `date` is whole-second by
/// construction — unlike the meeting leg's. That is load-bearing, not incidental:
/// it is the only thing that tells a `Meta`-derived date apart from a
/// filename-derived one, since both encode the same instant (see the `.000`
/// assertion below). Rendering it at millisecond precision would silently retire
/// that check.
///
/// The two prose turns together run past `chat::MIN_CHAT_DISTILL_CHARS` (400)
/// with room to spare. That gate is not decoration here: it is what decides
/// whether [`chats::list_undistilled_chats`] sees this transcript at all, so a
/// trim that drops under it turns the pre-assert below into a confusing
/// "expected one name, got none".
fn briarwood_chat_records() -> Vec<ChatRecord> {
    vec![
        ChatRecord::Meta {
            chat_id: "3c9a35a1-0000-4000-8000-000000000000".to_string(),
            model: "sonnet".to_string(),
            started_at: instant_on(14).to_rfc3339_opts(SecondsFormat::Secs, true),
        },
        ChatRecord::User {
            ts: "2026-07-14T14:04:02Z".to_string(),
            text: "We are choosing between the GreenFlow controller and the MERIDIAN retrofit \
                   for the irrigation loop this autumn. Which one did the vendor review land \
                   on, and can either of them read the flow meters that are already in the \
                   ground?"
                .to_string(),
        },
        ChatRecord::ToolUse {
            ts: "2026-07-14T14:04:05Z".to_string(),
            tool: "mcp__kodabi__search_notes".to_string(),
            input: json!({ "query": "irrigation controller vendor review" }),
            summary: "Searched notes for \"irrigation controller vendor review\"".to_string(),
        },
        ChatRecord::Assistant {
            ts: "2026-07-14T14:04:11Z".to_string(),
            text: "The vendor review shortlisted GreenFlow, mostly on install time. Neither \
                   controller talks to the meters you already have, so both quotes budget for \
                   a protocol bridge. The MERIDIAN retrofit reuses the current field wiring, \
                   which is most of why it comes in cheaper up front."
                .to_string(),
        },
        ChatRecord::Permission {
            ts: "2026-07-14T14:04:20Z".to_string(),
            request_id: "03eeffbb".to_string(),
            tool: "mcp__kodabi__file_note_to_project".to_string(),
            allowed: false,
            resolution: PermissionResolution::Cancelled,
        },
        ChatRecord::Error {
            ts: "2026-07-14T14:04:31Z".to_string(),
            message: "the turn failed".to_string(),
        },
    ]
}

/// The distilled output for [`briarwood_chat_records`]. Glossary-dense enough
/// that the real router files it into `Briarwood Golf` on vocabulary alone, and
/// carrying a summary phrase that appears **nowhere else** — not in the
/// transcript, the title, or the tags — so a later search that finds it can only
/// have matched the indexed note body.
///
/// The decision and the action item are deliberate. A chat is usually thinking
/// out loud and often has neither, but with neither, `render_body` omits both
/// sections and the "a chat's bullets never reach the index tables" assertion
/// below would have nothing to bite on.
///
/// The glossary channel clears the threshold on its own, so the corrections
/// channel sitting exactly at the shared-token floor is harmless. Don't "harden"
/// it by seeding the routing example's vocabulary into the summary — that games
/// a signal the decision doesn't rest on.
fn briarwood_chat_output() -> String {
    r#"{
        "title": "GreenFlow controller options",
        "summary": "Compared the GreenFlow controller against the MERIDIAN retrofit for the irrigation loop. Neither reads the meters already in the ground, so both quotes price around the pump house telemetry gap.",
        "decisions": ["Price the GreenFlow controller with the protocol bridge included"],
        "action_items": [
            {"owner": "Jane", "description": "ask MERIDIAN for a bridge line item", "due_date": "2026-08-07"}
        ],
        "open_questions": ["Does the TeeTrack tee sheet export need a matching schema change?"],
        "tags": ["irrigation", "research"]
    }"#
    .to_string()
}

#[test]
fn a_chat_transcript_distills_routes_writes_indexes_and_searches_back() {
    let vault = two_project_vault();
    let root = vault.path();
    let chat = write_chat(root, instant_on(14), &briarwood_chat_records());
    let chat_name = chat.file_name().unwrap().to_str().unwrap().to_string();

    // Before the chain runs no note claims the transcript, so the startup sweep
    // would take it. Without this the "no longer swept" assertion below would
    // also pass on a list that was empty for the wrong reason — a fixture that
    // drifted under the substance gate, say.
    assert_eq!(
        undistilled_chat_names(root),
        vec![chat_name.clone()],
        "an unclaimed, substantive transcript should be swept"
    );

    let decided = RefCell::new(None);
    let distilled = chat_distill::distill_chat(
        &MockRunner(briarwood_chat_output()),
        root,
        &chat,
        &recording_route(root, &decided),
    )
    .expect("distilling the chat");

    // --- routing → write -------------------------------------------------
    // The same router over the same vault signals, reached from the other raw
    // store: a chat is routed on its distilled text exactly as a meeting is.
    let routed = decided.borrow().clone().expect("the router ran");
    assert_eq!(routed.project(), "Briarwood Golf");
    let score = routed.confidence().expect("a routed note carries a score");
    assert!(
        score >= DEFAULT_THRESHOLD,
        "a glossary-rich chat should clear the routing threshold, got {score}"
    );
    assert_eq!(
        distilled.path.parent(),
        Some(root.join("Briarwood Golf").as_path()),
        "the note should land in the project routing chose"
    );

    // --- write → frontmatter ---------------------------------------------
    let mut index = NoteIndex::open_in_memory().unwrap();
    let note = index_written_note(&mut index, root, &distilled.path);
    assert_eq!(note.note_type, NoteType::Chat);
    assert_eq!(note.routing.project(), "Briarwood Golf");
    assert_close(
        note.routing.confidence().unwrap(),
        score,
        "the score routing computed should be the one on disk",
    );
    assert_eq!(
        note.source.as_yaml(),
        format!("chats/{chat_name}"),
        "the note must claim its transcript in the exact form the chat sweep joins on"
    );
    assert!(
        note.body.contains("pump house telemetry gap"),
        "body: {}",
        note.body
    );

    // The chat pass takes `date` from the transcript's own `Meta` record rather
    // than from the filename, renders it through the device's local offset, and
    // the index has to collapse that back to the instant the chat started.
    // Whole seconds, because that is the precision `chat_cmds` records
    // `started_at` at — so this is an exact string on any host, in any zone.
    let date_utc = normalize_date_to_utc(&note.date)
        .unwrap_or_else(|err| panic!("the writer's date {:?} must normalize: {err}", note.date));
    assert_eq!(
        date_utc, "2026-07-14T14:03:35Z",
        "the ordering key should be the instant the chat started"
    );
    // Which of the two recovery paths ran is legible only in the sub-second
    // field, and only on `date` itself: `normalize_date_to_utc` truncates to the
    // second, so the key above reads the same either way. The filename this
    // transcript was written under carries the capture instant's `.123`
    // milliseconds; the `Meta` record carries whole seconds. A `.000` here can
    // therefore only have come from `Meta` — and the seconds digit is stable
    // under every real zone offset, which are whole minutes.
    assert!(
        note.date.contains(":35.000"),
        "the date should come from the Meta record, not the filename: {}",
        note.date
    );

    // --- write → index → search ------------------------------------------
    let id = note.id.as_str();

    let row = index
        .get_note(id)
        .unwrap()
        .expect("the written note should be in the index");
    assert_eq!(row.note_type, IndexedNoteType::Chat);
    assert_eq!(row.project.as_deref(), Some("Briarwood Golf"));
    assert_eq!(row.date_utc, date_utc);
    assert!(
        index.note_has_chunks(id).unwrap(),
        "the note body should have been chunked and embedded"
    );

    // Where the two legs part. `meeting::meeting_facts_for` gates on
    // `NoteType::Meeting`, so a chat note is indexed with no facts at all — and
    // that holds even though its body renders a decision and an action item the
    // meeting leg would have parsed straight into those tables. A commitment
    // made in a chat is invisible to the outstanding-items surfaces today; this
    // is the test that makes changing that a decision rather than a side effect.
    assert!(
        note.body
            .contains("- [ ] Jane to ask MERIDIAN for a bridge line item by 2026-08-07."),
        "body: {}",
        note.body
    );
    assert!(
        index.get_meeting_facts(id).unwrap().is_none(),
        "a chat note carries no meeting facts"
    );
    assert!(
        index.get_action_items(id).unwrap().is_empty(),
        "and no action-item rows, however its body reads"
    );

    // A phrase that only ever existed in the distilled body.
    let hits = search(&index, "pump house telemetry gap").hits;
    assert_eq!(hits.len(), 1, "hits: {hits:?}");
    assert_eq!(hits[0].id, id);
    assert_eq!(hits[0].project.as_deref(), Some("Briarwood Golf"));
    assert_close(
        hits[0].confidence.unwrap(),
        score,
        "the score should survive all the way into a search hit",
    );

    // --- the claim seam ---------------------------------------------------
    assert!(
        undistilled_chat_names(root).is_empty(),
        "a distilled chat must stop being swept"
    );
}

/// The documented asymmetry between the two raw stores. `chats/` is a deliberate
/// sibling of `sessions/` precisely so a chat can never read as an unclaimed
/// capture: there is no needs-attention row for it and no retry button. Both
/// artifacts sit unclaimed here and each list sees exactly its own — an
/// assertion that would pass for the wrong reason if only one store were
/// populated, since either list returns empty for a directory that isn't there.
#[test]
fn an_undistilled_chat_never_surfaces_as_a_failed_capture() {
    let vault = two_project_vault();
    let root = vault.path();

    let session = write_session(root, instant_on(12), "budget-sync", &briarwood_segments());
    let chat = write_chat(root, instant_on(14), &briarwood_chat_records());
    let session_name = session.file_name().unwrap().to_str().unwrap().to_string();
    let chat_name = chat.file_name().unwrap().to_str().unwrap().to_string();

    assert_eq!(
        session_file_names(root),
        vec![session_name],
        "the needs-attention list is sessions/ only"
    );
    assert_eq!(
        undistilled_chat_names(root),
        vec![chat_name],
        "and the silent chat sweep is chats/ only"
    );
}

// ---------------------------------------------------------------------------
// the inbox-on-uncertainty leg
// ---------------------------------------------------------------------------

/// A capture that brushes one project's vocabulary without committing to it:
/// enough signal to score, not enough to clear the threshold.
fn ambiguous_output() -> String {
    r#"{
        "title": "Badge system cutover",
        "summary": "Facilities walked through the badge reader replacement in the north lobby. The vendor needs a week of lead time before the activation desk can issue new cards, and the loading dock stays on the legacy readers until then.",
        "decisions": ["Keep the loading dock on the legacy readers through the cutover"],
        "action_items": [],
        "open_questions": ["Who signs off on the vendor lead time?"],
        "tags": ["facilities"]
    }"#
    .to_string()
}

#[test]
fn an_unroutable_transcript_lands_in_inbox_with_its_score_and_is_still_searchable() {
    let vault = two_project_vault();
    let root = vault.path();
    let session = write_session(
        root,
        instant_on(13),
        "badge-cutover",
        &[
            segment(0, Channel::You, "where did we land on the badge readers"),
            segment(1, Channel::Them, "the vendor still owes us a lead time"),
        ],
    );

    let decided = RefCell::new(None);
    let distilled = distill::distill_session(
        &MockRunner(ambiguous_output()),
        root,
        &session,
        &recording_route(root, &decided),
    )
    .expect("distilling the session");

    let routed = decided.borrow().clone().expect("the router ran");
    assert_eq!(routed.project(), INBOX);
    let score = routed
        .confidence()
        .expect("an Inbox landing is always scored");
    assert!(
        score > 0.0 && score < DEFAULT_THRESHOLD,
        "an uncertain note should keep the score that fell short, got {score}"
    );
    assert_eq!(
        distilled.path.parent(),
        Some(root.join(INBOX).as_path()),
        "an under-threshold note belongs in the Inbox"
    );

    let mut index = NoteIndex::open_in_memory().unwrap();
    let note = index_written_note(&mut index, root, &distilled.path);
    let id = note.id.as_str();
    assert_close(
        note.routing.confidence().unwrap(),
        score,
        "the near-miss score must not be flattened on the way to disk",
    );

    // The index represents unfiled as `null`, not as the `Inbox` sentinel.
    let row = index.get_note(id).unwrap().expect("indexed");
    assert_eq!(row.project, None, "Inbox collapses to an unfiled project");
    assert_close(
        row.confidence.unwrap(),
        score,
        "an unfiled note still carries its routing score in the index",
    );

    let hits = search(&index, "loading dock legacy readers").hits;
    assert_eq!(hits.len(), 1, "hits: {hits:?}");
    assert_eq!(hits[0].id, id);
    assert_eq!(hits[0].project, None);

    // The one non-project root the claimed-source walk descends into. A
    // regression here would silently resurface every Inbox-routed capture as a
    // failed one.
    assert!(
        session_file_names(root).is_empty(),
        "an Inbox note claims its session just as a filed note does"
    );
}

// ---------------------------------------------------------------------------
// ordering
// ---------------------------------------------------------------------------

#[test]
fn notes_written_by_the_pipeline_order_chronologically_in_the_index() {
    let vault = two_project_vault();
    let root = vault.path();
    let mut index = NoteIndex::open_in_memory().unwrap();

    let earlier = write_session(root, instant_on(12), "first", &briarwood_segments());
    let later = write_session(root, instant_on(15), "second", &briarwood_segments());

    // Same fixture output bar the title, so the two notes differ only in the
    // date the writer derived from each capture — which is the thing under test.
    let renamed =
        briarwood_output().replace("Irrigation rebuild kickoff", "Irrigation rebuild recap");
    let decided = RefCell::new(None);

    let first = distill::distill_session(
        &MockRunner(briarwood_output()),
        root,
        &earlier,
        &recording_route(root, &decided),
    )
    .expect("distilling the earlier session");
    let second = distill::distill_session(
        &MockRunner(renamed),
        root,
        &later,
        &recording_route(root, &decided),
    )
    .expect("distilling the later session");

    let first_note = index_written_note(&mut index, root, &first.path);
    let second_note = index_written_note(&mut index, root, &second.path);
    assert_ne!(
        first_note.date, second_note.date,
        "the two captures should yield different note dates"
    );

    let recent = index
        .recent_notes(&ProjectScope::resolve(None, true), 10)
        .expect("listing recent notes");
    let ids: Vec<&str> = recent.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![second_note.id.as_str(), first_note.id.as_str()],
        "the later capture should sort first"
    );
    assert!(
        recent[0].date_utc > recent[1].date_utc,
        "the UTC ordering key derived from the writer's dates must be monotonic: {:?}",
        recent.iter().map(|r| &r.date_utc).collect::<Vec<_>>()
    );
}

/// The two legs recover their `date` from different places — a meeting's from
/// the session filename, a chat's from the transcript's own `Meta` record — and
/// both then pass through the same local-offset rendering. Nothing else checks
/// that the two paths land on mutually comparable ordering keys, which is the
/// only reason one vault's notes sort correctly against each other at all.
#[test]
fn a_chat_note_and_a_meeting_note_order_against_each_other() {
    let vault = two_project_vault();
    let root = vault.path();
    let mut index = NoteIndex::open_in_memory().unwrap();

    let session = write_session(root, instant_on(12), "budget-sync", &briarwood_segments());
    let chat = write_chat(root, instant_on(14), &briarwood_chat_records());

    let decided = RefCell::new(None);
    let meeting_note = distill::distill_session(
        &MockRunner(briarwood_output()),
        root,
        &session,
        &recording_route(root, &decided),
    )
    .expect("distilling the session");
    let chat_note = chat_distill::distill_chat(
        &MockRunner(briarwood_chat_output()),
        root,
        &chat,
        &recording_route(root, &decided),
    )
    .expect("distilling the chat");

    let meeting_note = index_written_note(&mut index, root, &meeting_note.path);
    let chat_note = index_written_note(&mut index, root, &chat_note.path);

    let recent = index
        .recent_notes(&ProjectScope::resolve(None, true), 10)
        .expect("listing recent notes");
    let ids: Vec<&str> = recent.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![chat_note.id.as_str(), meeting_note.id.as_str()],
        "the later artifact should sort first whichever store it came from"
    );
    assert!(
        recent[0].date_utc > recent[1].date_utc,
        "two date-recovery paths must yield comparable ordering keys: {:?}",
        recent.iter().map(|r| &r.date_utc).collect::<Vec<_>>()
    );
}
