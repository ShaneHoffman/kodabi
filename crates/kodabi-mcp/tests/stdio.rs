//! End-to-end checks of the built binary: a real stdio handshake, real
//! `tools/call`s, and — critically — that stdout carries only JSON-RPC frames
//! (stderr-only diagnostics discipline).
//!
//! Two fidelities of the same seam. The first seeds the index directly, so it
//! can pin exact fixture values (an always-overdue action item) cheaply. The
//! second grows the index the way the app grows it — real notes written through
//! `note::write_note`, then indexed through the same
//! `IndexedNote::from_note` + `meeting_facts_for` + `embed::index_note` trio
//! `src-tauri/src/note_cmds.rs` runs — so it proves that what the writer put on
//! disk is what MCP serves back. That is the Phase 2 → Phase 3 handoff, and the
//! place a writer/indexer disagreement would hide: path separators, the
//! `source:` form, the title fallback, date normalization.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use chrono::{TimeZone, Utc};
use kodabi_core::device::DeviceId;
use kodabi_core::index::{IndexedNote, NoteIndex, NoteType};
use kodabi_core::meeting::{self, ActionItemFact, MeetingFacts};
use kodabi_core::note::{self, Note, NoteId, Routing, Source, Tag};
use kodabi_core::raw_session::{self, TranscriptSegment};
use kodabi_core::transcription::Channel;
use kodabi_core::{embed, vault};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Spawns the built binary against `index_db` + `kb_root`, feeds it `requests`
/// (newline-delimited JSON-RPC), and returns the parsed responses.
///
/// Dropping stdin is what ends the run: the server loops until EOF, so without
/// the close it would block forever and so would `wait_with_output`. Keep
/// `requests` well under the OS pipe buffer (~4 KiB on Windows) — the whole
/// blob is written before anything is read, so a child stalled on a full stdout
/// pipe would stop draining stdin and wedge both sides. A larger blob needs a
/// stdin-writer thread instead.
fn run_server(index_db: &Path, kb_root: &Path, requests: &str) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kodabi-mcp"))
        .env("KODABI_INDEX_DB", index_db)
        .env("KODABI_KB_ROOT", kb_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        // Dropping stdin closes the pipe, so the server hits EOF and exits.
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(requests.as_bytes()).unwrap();
    }

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "server exited unsuccessfully: {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    // Stdout purity: every non-empty line must parse as a JSON-RPC frame. It
    // lives here so both tests get the check by construction.
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("non-JSON on stdout: {line:?} ({error})"))
        })
        .collect()
}

/// The response carrying `id`.
fn response(responses: &[Value], id: i64) -> &Value {
    responses
        .iter()
        .find(|response| response["id"] == id)
        .unwrap_or_else(|| panic!("no response for id {id}"))
}

/// The `structuredContent` of a *successful* tool call.
///
/// Asserting the success envelope at every use site is what stops a business
/// error being mistaken for an empty result: a business error carries no
/// `structuredContent` key at all, so indexing into it would silently yield
/// `null` rather than failing.
fn structured(responses: &[Value], id: i64) -> &Value {
    let result = &response(responses, id)["result"];
    assert_eq!(result["isError"], Value::Bool(false), "id {id}: {result}");
    &result["structuredContent"]
}

// ---------------------------------------------------------------------------
// Hand-seeded index
// ---------------------------------------------------------------------------

#[test]
fn stdio_server_handshakes_reads_and_writes_with_clean_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.db");
    let vault = dir.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    std::fs::create_dir(vault.join("Growth")).unwrap();

    // Seed the index, then drop the connection so the child opens it cleanly.
    {
        let mut index = NoteIndex::open(&index_path).unwrap();
        index
            .upsert_note(&IndexedNote {
                id: "n_note01".to_string(),
                path: "Growth/plan.md".to_string(),
                title: "Plan".to_string(),
                note_type: NoteType::Meeting,
                project: Some("Growth".to_string()),
                date: "2026-07-10".to_string(),
                tags: vec![],
                source: "manual".to_string(),
                confidence: None,
                category: None,
                category_confidence: None,
                tracking: None,
                body: "The quarterly plan.".to_string(),
                // A meeting with one overdue action item, so the milestone tool
                // (`list_outstanding_items`) has something real to serve
                // through the built binary.
                meeting: Some(MeetingFacts {
                    duration_seconds: Some(900),
                    speaker_count: Some(2),
                    decisions: vec!["Ship the plan".to_string()],
                    action_items: vec![ActionItemFact {
                        id: "a_memo01".to_string(),
                        description: "send the memo".to_string(),
                        owner: "You".to_string(),
                        // Safely in the past → `overdue` whenever this runs.
                        due_date: Some("2020-01-01".to_string()),
                        done: false,
                        extracted_date: Some("2026-07-10".to_string()),
                    }],
                }),
            })
            .unwrap();
    }

    let requests = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"list_projects\",\"arguments\":{}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"add_glossary_term\",\"arguments\":{\"project\":\"Growth\",\"term\":\"MERIDIAN\",\"definition\":\"A migration project.\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"list_outstanding_items\",\"arguments\":{\"project\":\"Growth\"}}}\n",
    );
    let responses = run_server(&index_path, &vault, requests);

    // Four responses: initialize (id 1), list_projects (id 2),
    // add_glossary_term (id 3), and list_outstanding_items (id 4). The
    // notification draws none.
    assert_eq!(responses.len(), 4);

    let init = response(&responses, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "kodabi");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let projects = structured(&responses, 2)["projects"].as_array().unwrap();
    assert!(projects.iter().any(|project| project["slug"] == "Growth"));

    // The write tool round-trips over stdio: a new glossary term is created.
    let added = structured(&responses, 3);
    assert_eq!(added["created"], true);
    assert_eq!(added["term"]["term"], "MERIDIAN");

    // The write landed on disk in the project's glossary file.
    let glossary = std::fs::read_to_string(vault.join("Growth").join("_glossary.yml")).unwrap();
    assert!(glossary.contains("MERIDIAN"));

    // The milestone read ("what's outstanding on <project>?") serves the
    // action item straight out of the file-backed index, over the real binary.
    let outstanding = structured(&responses, 4);
    let items = outstanding["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "a_memo01");
    assert_eq!(items[0]["status"], "overdue");
    assert_eq!(items[0]["source"]["id"], "n_note01");
    assert_eq!(outstanding["summary"]["overdue"], 1);
}

// ---------------------------------------------------------------------------
// Vault-grown index
// ---------------------------------------------------------------------------

/// Writes `note` into the vault and indexes it through the *same* trio the
/// app's write path uses (`src-tauri/src/note_cmds.rs`): `IndexedNote::from_note`
/// over `vault::effective_title`, then `meeting::meeting_facts_for`, then
/// `embed::index_note`. Returns the KB-relative, forward-slashed path the index
/// now holds, so callers assert against the real value instead of re-deriving
/// it.
///
/// `filename_seed` is deliberately a separate argument from the note's
/// frontmatter `title`: the writer slugs the filename from the seed, while a
/// reader prefers the stored title. A note can carry one without the other, and
/// closing that gap is exactly what `effective_title` is for.
fn write_and_index(
    index: &mut NoteIndex,
    vault: &Path,
    note: &Note,
    filename_seed: Option<&str>,
) -> String {
    let path = note::write_note(vault, note, filename_seed).unwrap();
    // The one place a Windows `\` would leak into the wire contract.
    let relative = path
        .strip_prefix(vault)
        .unwrap_or(&path)
        .to_string_lossy()
        .replace('\\', "/");

    let mut indexed = IndexedNote::from_note(note, &vault::effective_title(note, &path), &relative);
    indexed.meeting = meeting::meeting_facts_for(note, vault);
    // `None` embedder — a plain upsert, the same FTS-only service level the MCP
    // wrapper runs at (`tools/search_notes.rs`), and why this test needs no
    // model and is not `#[ignore]`d.
    embed::index_note(index, &indexed, None).unwrap();
    relative
}

#[test]
fn vault_grown_index_serves_what_the_markdown_on_disk_says() {
    let dir = tempfile::tempdir().unwrap();
    // The index lives outside the vault: it is machine-local and disposable,
    // and keeping it out also keeps its `-wal`/`-shm` files out of the project
    // scan that backs `list_projects`.
    let index_path = dir.path().join("index.db");
    let vault = dir.path().join("vault");
    std::fs::create_dir(&vault).unwrap();

    // A real session transcript, so the meeting's raw-artifact `source:` has
    // something behind it. Without the file, `duration_seconds`/`speaker_count`
    // come back null and the `source:` assertion proves nothing — with it, they
    // are non-null only if `kb_root.join(source)` actually resolved.
    let session = raw_session::write_raw_session(
        &vault.join("raw"),
        Utc.with_ymd_and_hms(2026, 7, 10, 3, 0, 0).unwrap(),
        &DeviceId::parse("k4m2xp7q").unwrap(),
        Some("briarwood golf q3 sync"),
        &[
            TranscriptSegment {
                index: 0,
                channel: Channel::You,
                speaker: None,
                start_ms: 0,
                end_ms: 300_000,
                text: "Walking the Q3 budget.".to_string(),
            },
            TranscriptSegment {
                index: 1,
                channel: Channel::Them,
                speaker: None,
                start_ms: 300_000,
                end_ms: 900_000,
                text: "And the contractor shortlist.".to_string(),
            },
        ],
    )
    .unwrap();
    // A raw-artifact `source:` is repo-relative with `/` separators —
    // `Source::parse` rejects a `\` outright, so this is the contract, not a
    // convenience. `raw/` is a reserved root dir, so project discovery skips it.
    let session_rel = session
        .strip_prefix(&vault)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    assert!(session_rel.starts_with("raw/"), "{session_rel}");

    // The three dates are load-bearing. Sorted as raw strings they run
    // chat > meeting > note; sorted as UTC instants they run
    // meeting > chat > note. `recent_notes` is `ORDER BY date_utc DESC`, so the
    // order asserted below can only come out right if `upsert_note`'s
    // normalization actually ran — while each `date` served back stays the
    // verbatim offset-bearing string the frontmatter holds.
    let meeting_note = Note::new(
        NoteId::parse("n_meet01").unwrap(),
        note::NoteType::Meeting,
        Routing::Routed {
            project: "Growth".to_string(),
            confidence: 0.94,
        },
        "2026-07-09T20:00:00-07:00", // 2026-07-10T03:00:00Z
        vec![
            Tag::parse("budgeting").unwrap(),
            Tag::parse("phase-2").unwrap(),
        ],
        Source::parse(&session_rel).unwrap(),
        concat!(
            "# Summary\n\n",
            "Briarwood Golf walked the Q3 budget and the contractor shortlist.\n\n",
            "## Decisions\n\n",
            "- Hold the Q3 budget flat\n",
            "- Shortlist three contractors before the next sync\n\n",
            "## Action items\n\n",
            // Safely past → `overdue` whenever this runs.
            "- [ ] Jane to circulate the shortlist by 2020-01-01.\n",
            // No due date → `open` whenever this runs. Both statuses are
            // clock-independent, and they have to be: the server derives them
            // against `Local::now()`.
            "- [ ] Unassigned to confirm the sprinkler quote.",
        ),
    )
    .unwrap()
    .with_title(Some("Briarwood Golf Q3 sync".to_string()));

    // Deliberately *no* `title:` key, and filed in a sub-project: this one note
    // carries both the title fallback (its indexed title can only come from the
    // de-slugged filename stem) and the nested path separator.
    let plain_note = Note::new(
        NoteId::parse("n_note01").unwrap(),
        note::NoteType::Note,
        Routing::Manual {
            project: "Growth/Q3".to_string(),
        },
        "2026-07-08",
        vec![Tag::parse("vendors").unwrap()],
        Source::parse("quick-capture").unwrap(),
        "Briarwood asked for an irrigation vendor comparison before the Q3 close.",
    )
    .unwrap();

    let chat_note = Note::new(
        NoteId::parse("n_chat01").unwrap(),
        note::NoteType::Chat,
        Routing::Routed {
            project: "Growth".to_string(),
            confidence: 0.62,
        },
        "2026-07-10T03:00:00+02:00", // 2026-07-10T01:00:00Z
        vec![],
        Source::parse("chat").unwrap(),
        // A chat body carries the same rendered grammar a meeting's does, and it
        // is indexed the same way — so this note's commitment has to survive the
        // whole trip through the built binary. Undated → `open` whenever this
        // runs, like the meeting's second item.
        concat!(
            "Asked about Briarwood pricing tiers and got the standard rate card back.\n\n",
            "## Decisions\n\n",
            "- Price the protocol bridge as a separate line.\n\n",
            "## Action items\n\n",
            "- [ ] Jane to ask MERIDIAN for a bridge line item.",
        ),
    )
    .unwrap()
    .with_title(Some("Briarwood pricing questions".to_string()));

    // Grow the index from the notes on disk, then drop the connection so the
    // child opens the file cleanly.
    let (meeting_path, plain_path, chat_path) = {
        let mut index = NoteIndex::open(&index_path).unwrap();
        (
            write_and_index(
                &mut index,
                &vault,
                &meeting_note,
                Some("Briarwood Golf Q3 sync"),
            ),
            write_and_index(
                &mut index,
                &vault,
                &plain_note,
                Some("Irrigation vendor shortlist"),
            ),
            write_and_index(
                &mut index,
                &vault,
                &chat_note,
                Some("Briarwood pricing questions"),
            ),
        )
    };

    // The writer's own path derivation, asserted before anything else reads it.
    assert_eq!(meeting_path, "Growth/briarwood-golf-q3-sync.md");
    assert_eq!(plain_path, "Growth/Q3/irrigation-vendor-shortlist.md");
    assert_eq!(chat_path, "Growth/briarwood-pricing-questions.md");

    // `Briarwood` is a single verbatim token in all three notes. FTS5 runs the
    // default `unicode61` tokenizer with no stemmer, so a query term has to
    // match a real token exactly — `contractors` would not find `contractor`.
    let requests = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"search_notes\",\"arguments\":{\"query\":\"Briarwood\",\"project\":\"Growth\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"get_note\",\"arguments\":{\"id\":\"n_meet01\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"get_note\",\"arguments\":{\"id\":\"n_note01\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":5,\"method\":\"tools/call\",\"params\":{\"name\":\"get_note\",\"arguments\":{\"id\":\"n_chat01\"}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"list_projects\",\"arguments\":{}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"get_project_context\",\"arguments\":{\"project\":\"Growth\",\"include_descendants\":true}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"get_project_context\",\"arguments\":{\"project\":\"Growth\"}}}\n",
    );
    let responses = run_server(&index_path, &vault, requests);
    assert_eq!(responses.len(), 8);

    let init = response(&responses, 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "kodabi");

    // --- id 2: search_notes ------------------------------------------------

    let search = structured(&responses, 2);
    let hits = search["hits"].as_array().unwrap();
    // Three hits: the project filter resolves to a subtree scope, so the
    // `Growth/Q3` note comes along.
    assert_eq!(hits.len(), 3, "{search}");
    assert_eq!(search["page"]["has_more"], false);

    // Hit order is bm25-derived (the vector arm is off), so compare the set of
    // paths, not their positions. A Windows `\` anywhere in the write→index
    // path fails right here.
    let mut paths: Vec<&str> = hits
        .iter()
        .map(|hit| hit["path"].as_str().unwrap())
        .collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "Growth/Q3/irrigation-vendor-shortlist.md",
            "Growth/briarwood-golf-q3-sync.md",
            "Growth/briarwood-pricing-questions.md",
        ]
    );

    let meeting_hit = hits.iter().find(|hit| hit["id"] == "n_meet01").unwrap();
    assert_eq!(meeting_hit["title"], "Briarwood Golf Q3 sync");
    assert_eq!(meeting_hit["project"], "Growth");
    assert_eq!(meeting_hit["type"], "meeting");
    // The frontmatter date, verbatim: the offset survives disk → index → wire.
    assert_eq!(meeting_hit["date"], "2026-07-09T20:00:00-07:00");
    // The `source:` form: a repo-relative raw-artifact path, not a keyword.
    assert_eq!(meeting_hit["source"], session_rel.as_str());
    assert_eq!(meeting_hit["tags"], json!(["budgeting", "phase-2"]));
    assert_eq!(meeting_hit["confidence"].as_f64(), Some(0.94));
    assert!(meeting_hit["snippet"]
        .as_str()
        .unwrap()
        .contains("Briarwood"));

    let plain_hit = hits.iter().find(|hit| hit["id"] == "n_note01").unwrap();
    // The title fallback: the file carries no `title:` key, so the only source
    // for this string is the filename stem with its hyphens de-slugged.
    assert_eq!(plain_hit["title"], "irrigation vendor shortlist");
    assert_eq!(plain_hit["project"], "Growth/Q3");
    // Manual routing carries no score.
    assert_eq!(plain_hit["confidence"], Value::Null);

    // --- id 3: get_note on the meeting -------------------------------------

    let fetched = structured(&responses, 3);
    assert_eq!(fetched["note"]["path"], meeting_path.as_str());
    assert_eq!(fetched["note"]["source"], session_rel.as_str());

    let meeting_meta = &fetched["meeting"];
    // Non-null only because the `source:` path resolved under the KB root — the
    // assertion a `\` in either the note path or the session path would break.
    assert_eq!(meeting_meta["duration_seconds"], 900);
    assert_eq!(meeting_meta["speaker_count"], 2);
    assert_eq!(
        meeting_meta["decisions"],
        json!([
            "Hold the Q3 budget flat",
            "Shortlist three contractors before the next sync",
        ])
    );
    assert_eq!(meeting_meta["action_item_count"], 2);

    // Action items come back in body order.
    let items = fetched["action_items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items[0]["id"].as_str().unwrap().starts_with("a_"));
    assert_eq!(items[0]["owner"], "Jane");
    assert_eq!(items[0]["description"], "circulate the shortlist");
    assert_eq!(items[0]["due_date"], "2020-01-01");
    assert_eq!(items[0]["status"], "overdue");
    // Date normalization, in the other direction: the extracted date is the
    // note's *local* calendar day, never its UTC day (2026-07-10).
    assert_eq!(items[0]["extracted_date"], "2026-07-09");
    assert_eq!(items[1]["owner"], "Unassigned");
    assert_eq!(items[1]["due_date"], Value::Null);
    assert_eq!(items[1]["status"], "open");

    // The body MCP serves is what a fresh parse of the file on disk yields.
    // Comparing against the re-parsed note rather than the raw file sidesteps
    // `Note::new`'s body trimming.
    let on_disk =
        std::fs::read_to_string(vault.join("Growth").join("briarwood-golf-q3-sync.md")).unwrap();
    assert_eq!(
        fetched["body_markdown"],
        Note::from_markdown(&on_disk).unwrap().body.as_str()
    );
    assert!(on_disk.contains("title: Briarwood Golf Q3 sync"));
    assert!(on_disk.contains(&format!("source: {session_rel}")));

    // --- id 4: get_note on the plain note ----------------------------------

    let plain = structured(&responses, 4);
    assert_eq!(plain["note"]["path"], plain_path.as_str());
    assert_eq!(plain["note"]["title"], "irrigation vendor shortlist");
    assert_eq!(plain["note"]["project"], "Growth/Q3");
    assert_eq!(plain["note"]["type"], "note");
    assert_eq!(plain["note"]["source"], "quick-capture");
    assert_eq!(plain["note"]["date"], "2026-07-08");
    assert_eq!(plain["note"]["confidence"], Value::Null);
    assert_eq!(plain["meeting"], Value::Null);
    assert!(plain["action_items"].as_array().unwrap().is_empty());

    // The fallback only means anything if the file genuinely lacks the key.
    let plain_on_disk = std::fs::read_to_string(
        vault
            .join("Growth")
            .join("Q3")
            .join("irrigation-vendor-shortlist.md"),
    )
    .unwrap();
    assert!(
        !plain_on_disk.contains("title:"),
        "fixture must carry no title key: {plain_on_disk}"
    );
    assert_eq!(
        plain["body_markdown"],
        Note::from_markdown(&plain_on_disk).unwrap().body.as_str()
    );

    // --- id 5: get_note on the chat note ------------------------------------

    let chat = structured(&responses, 5);
    assert_eq!(chat["note"]["path"], chat_path.as_str());
    assert_eq!(chat["note"]["type"], "chat");
    assert_eq!(chat["note"]["source"], "chat");
    // The `tags:` key is absent from the frontmatter, never `tags: []`.
    assert_eq!(chat["note"]["tags"], json!([]));
    assert_eq!(chat["note"]["confidence"].as_f64(), Some(0.62));
    // `meeting` stays null for a chat: `MeetingMeta` leads with the two session
    // scalars, which a chat can never carry.
    assert_eq!(chat["meeting"], Value::Null);
    // Its action items, though, come back like any other note's — through the
    // real binary, with `status` derived against `Local::now()` server-side.
    let chat_items = chat["action_items"].as_array().unwrap();
    assert_eq!(chat_items.len(), 1);
    assert_eq!(chat_items[0]["owner"], "Jane");
    assert_eq!(
        chat_items[0]["description"],
        "ask MERIDIAN for a bridge line item"
    );
    assert_eq!(chat_items[0]["due_date"], Value::Null);
    assert_eq!(chat_items[0]["status"], "open");
    assert_eq!(chat_items[0]["source"]["id"], "n_chat01");

    // --- id 6: list_projects ------------------------------------------------

    let projects = structured(&responses, 6)["projects"].as_array().unwrap();
    // Sorted by display name; `raw/` is a reserved root dir and never appears.
    let slugs: Vec<&str> = projects
        .iter()
        .map(|project| project["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["Growth", "Growth/Q3"]);

    let growth = &projects[0];
    // Hash- and mtime-derived respectively: assert shape, never the value.
    assert!(growth["id"].as_str().unwrap().starts_with("p_"));
    assert!(growth["last_activity"].is_string());
    assert_eq!(growth["display_name"], "Growth");
    assert_eq!(growth["parent"], Value::Null);
    // Disk-derived and direct-children only, so the `Growth/Q3` note is
    // deliberately not counted here.
    assert_eq!(growth["note_count"], 2);
    assert_eq!(growth["meeting_count"], 1);

    let q3 = &projects[1];
    assert_eq!(q3["display_name"], "Q3");
    assert_eq!(q3["parent"], "Growth");
    assert_eq!(q3["note_count"], 1);
    assert_eq!(q3["meeting_count"], 0);

    // --- id 7: get_project_context over the subtree --------------------------

    let context = structured(&responses, 7);
    assert_eq!(context["project"]["slug"], "Growth");
    // `Project.note_count` stays "directly in this project" by definition, so
    // it deliberately differs from `counts.notes` below.
    assert_eq!(context["project"]["note_count"], 2);

    let counts = &context["counts"];
    assert_eq!(counts["notes"], 3);
    assert_eq!(counts["meetings"], 1);
    // Index-derived over the subtree, and the chat note is counted as a chat.
    assert_eq!(
        counts["notes_by_type"],
        json!({ "meeting": 1, "note": 1, "chat": 1 })
    );
    // Two open: the meeting's undated item and the chat's. A chat's commitments
    // count here exactly as a meeting's do.
    assert_eq!(counts["outstanding_open"], 2);
    assert_eq!(counts["outstanding_overdue"], 1);
    assert_eq!(counts["glossary_terms"], 0);
    assert_eq!(context["description"], Value::Null);

    // Date normalization: ordered by `date_utc` DESC, the meeting (03:00Z) is
    // newer than the chat (01:00Z) even though their raw strings sort the other
    // way. A write path that skipped normalization inverts these two.
    let recent: Vec<&str> = context["recent_notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note["id"].as_str().unwrap())
        .collect();
    assert_eq!(recent, ["n_meet01", "n_chat01", "n_note01"]);
    // ...and the raw offsets still come back untouched.
    assert_eq!(
        context["recent_notes"][0]["date"],
        "2026-07-09T20:00:00-07:00"
    );
    assert_eq!(
        context["recent_notes"][1]["date"],
        "2026-07-10T03:00:00+02:00"
    );

    // Dated items sort ahead of undated ones, so the overdue one leads.
    assert_eq!(context["outstanding"][0]["status"], "overdue");
    assert_eq!(context["outstanding"][0]["source"]["id"], "n_meet01");
    assert_eq!(
        context["outstanding"][0]["source"]["path"],
        meeting_path.as_str()
    );
    // This is the one read tool with no pagination envelope.
    assert!(context.get("page").is_none());

    // --- id 8: get_project_context at its default ---------------------------

    // Unlike every other tool, this one defaults `include_descendants` to
    // false, so the `Growth/Q3` note drops out of the counts.
    let narrow = structured(&responses, 8);
    assert_eq!(narrow["counts"]["notes"], 2);
    assert_eq!(
        narrow["counts"]["notes_by_type"],
        json!({ "meeting": 1, "note": 0, "chat": 1 })
    );
}

// ---------------------------------------------------------------------------
// The commitment ledger, over a real process boundary
// ---------------------------------------------------------------------------

/// Like [`run_server`], but also wires `KODABI_LEDGER_DB` — the seam the
/// commitment tools need and the other eight do not.
fn run_server_with_ledger(
    index_db: &Path,
    kb_root: &Path,
    ledger_db: &Path,
    requests: &str,
) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_kodabi-mcp"))
        .env("KODABI_INDEX_DB", index_db)
        .env("KODABI_KB_ROOT", kb_root)
        .env("KODABI_LEDGER_DB", ledger_db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(requests.as_bytes()).unwrap();
    }

    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "server exited unsuccessfully: {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("non-JSON on stdout: {line:?} ({error})"))
        })
        .collect()
}

/// A vault + index + ledger describing the same two commitments, grown the way
/// the app grows them: a real note on disk, indexed from it, and the ledger
/// synced from the ids that note's body derives.
///
/// Returns the ledger path and the two action-item ids (tracked, untracked).
fn seed_ledger_fixture(vault: &Path, index: &mut NoteIndex, ledger_db: &Path) -> (String, String) {
    use kodabi_core::ledger::{Ledger, NoteSync, OwnerIdentity, UntrackedVia};

    let body = concat!(
        "# Summary\n\n",
        "We met.\n\n",
        "## Action items\n\n",
        "- [ ] Priya to draft the plan.\n",
        "- [ ] Jane to chase the invoice.\n",
    );
    let note = Note::new(
        NoteId::parse("n_meet09").unwrap(),
        note::NoteType::Meeting,
        Routing::Manual {
            project: "Growth".to_string(),
        },
        "2026-07-10",
        vec![],
        Source::parse("manual").unwrap(),
        body,
    )
    .unwrap();
    write_and_index(index, vault, &note, Some("commitments"));

    let facts = meeting::meeting_facts_for(&note, vault).expect("a meeting carries facts");
    assert_eq!(facts.action_items.len(), 2);
    let tracked = facts.action_items[0].id.clone();
    let untracked_item = facts.action_items[1].id.clone();

    let mut ledger = Ledger::open(ledger_db).unwrap();
    let created = ledger
        .sync_note_items(&NoteSync {
            note_id: "n_meet09",
            project: "Growth",
            note_date_utc: "2026-07-10T00:00:00Z",
            items: &facts.action_items,
            link_hints: &[],
            note_override: None,
            category_default: None,
            identity: &OwnerIdentity::default(),
            now: "2026-07-13T00:00:00Z",
        })
        .unwrap()
        .created;
    assert_eq!(created.len(), 2);

    // The second is taken out of the working set by hand, so the fixture holds
    // one commitment the ledger tracks and one it deliberately does not.
    let second = ledger
        .entry_for_item("n_meet09", &untracked_item)
        .unwrap()
        .expect("the second item minted an entry");
    ledger
        .untrack(
            &second.entry_id,
            UntrackedVia::Manual,
            "2026-07-15T00:00:00Z",
        )
        .unwrap();

    (tracked, untracked_item)
}

/// The whole point of the ticket, over a real process boundary: chat's answer
/// about what is outstanding matches what the app is tracking, and a mark-done
/// from chat lands in both stores.
#[test]
fn stdio_server_serves_commitments_and_marks_one_done() {
    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.db");
    let ledger_path = dir.path().join("ledger.db");
    let vault = dir.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    std::fs::create_dir(vault.join("Growth")).unwrap();

    let (tracked, untracked_item) = {
        let mut index = NoteIndex::open(&index_path).unwrap();
        seed_ledger_fixture(&vault, &mut index, &ledger_path)
    };

    let requests = format!(
        "{}\n{}\n{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
               "params":{"name":"list_commitments","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
               "params":{"name":"update_action_item",
                         "arguments":{"note_id":"n_meet09","item_id":tracked,"done":true}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
               "params":{"name":"list_commitments","arguments":{}}}),
    );
    let responses = run_server_with_ledger(&index_path, &vault, &ledger_path, &requests);

    // The tracked commitment is served; the untracked one is not.
    let before = structured(&responses, 1);
    let rows = before["commitments"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "only the tracked commitment: {before}");
    assert_eq!(rows[0]["item"]["item_id"], tracked);
    assert_eq!(rows[0]["state"], "open");
    assert_eq!(before["summary"]["total"], 1);

    // The write reports both halves.
    let written = structured(&responses, 2);
    assert_eq!(written["note_outcome"], "updated");
    assert_eq!(written["entry"]["state"], "closed");
    assert_eq!(written["entry"]["closed_via"], "manual");

    // And the next read agrees, so chat cannot contradict itself either.
    let after = structured(&responses, 3);
    assert!(after["commitments"].as_array().unwrap().is_empty());

    // The durable stores really moved: the checkbox on disk, and the entry in a
    // ledger this test opens for itself after the child exited.
    let markdown = std::fs::read_to_string(vault.join("Growth").join("commitments.md")).unwrap();
    assert!(
        markdown.contains("- [x] Priya to draft the plan."),
        "{markdown}"
    );
    assert!(
        markdown.contains("- [ ] Jane to chase the invoice."),
        "the untargeted line is byte-preserved: {markdown}"
    );

    let ledger = kodabi_core::ledger::Ledger::open(&ledger_path).unwrap();
    let entry = ledger
        .entry_for_item("n_meet09", &tracked)
        .unwrap()
        .expect("the entry survived");
    assert_eq!(entry.state, kodabi_core::ledger::EntryState::Closed);
    assert!(entry.touched, "a person made this call, not a machine");

    // The vault snapshot was refreshed by the process that dirtied it, so a
    // rebuild-from-empty would not lose the judgement.
    assert!(
        vault.join("Growth").join("_ledger.yml").is_file(),
        "the sidecar flushed its own snapshot"
    );
    let _ = untracked_item;
}

/// The ledger now has two writers: the app's worker and this sidecar. Without a
/// `busy_timeout` the loser of a race fails instantly with SQLITE_BUSY.
#[test]
fn a_write_waits_out_another_processs_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let index_path = dir.path().join("index.db");
    let ledger_path = dir.path().join("ledger.db");
    let vault = dir.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    std::fs::create_dir(vault.join("Growth")).unwrap();

    let tracked = {
        let mut index = NoteIndex::open(&index_path).unwrap();
        seed_ledger_fixture(&vault, &mut index, &ledger_path).0
    };

    // Stand in for the app's worker mid-write: an exclusive transaction held
    // for a beat after the child is already trying to write.
    let holder = rusqlite::Connection::open(&ledger_path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        holder.execute_batch("COMMIT").unwrap();
    });

    let requests = format!(
        "{}\n",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
               "params":{"name":"update_action_item",
                         "arguments":{"note_id":"n_meet09","item_id":tracked,"done":true}}}),
    );
    let responses = run_server_with_ledger(&index_path, &vault, &ledger_path, &requests);
    release.join().unwrap();

    // It waited rather than failing: the write landed.
    let written = structured(&responses, 1);
    assert_eq!(written["entry"]["state"], "closed");
}
