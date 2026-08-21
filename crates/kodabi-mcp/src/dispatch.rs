//! JSON-RPC method routing: the `initialize` handshake, `tools/list`,
//! `tools/call`, `ping`, and the notification/unknown-method rules.

use serde_json::{json, Value};

use crate::protocol::{error_response, success_response, RpcError};
use crate::schemas;
use crate::server::Server;
use crate::tools;

/// The MCP protocol version this server implements (the revision
/// `docs/MCP_TOOL_SURFACE.md` was verified against).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The server instructions surfaced at `initialize`. Must stay under 2 KB —
/// Claude Code truncates this (and every tool description) at that size, which
/// would silently drop the "when to use this" guidance a client relies on.
const INSTRUCTIONS: &str = "Kodabi tools over your local knowledge base of distilled, routed Markdown notes. Read: search_notes for hybrid full-text and semantic retrieval (ranked snippets), get_note to read a hit in full by its stable id, get_meeting_transcript for a meeting's per-channel transcript segments, list_commitments for what the user is tracking (the commitment ledger, after their own closures and waivers), list_outstanding_items for the raw extracted action items behind it, list_projects to resolve a project name to its slug before filtering, and get_project_context for a one-call briefing on a project. For any question about what is outstanding, owed, or being waited on, prefer list_commitments: list_outstanding_items still reports items the user has settled. Write: file_note_to_project re-files a note while preserving its stable id, add_glossary_term upserts a project glossary term, and update_action_item ticks or unticks an item, closing or reopening the tracked commitment with it. Ids and slugs are the handles: a note id looks like n_a1b2c3, a project slug is a folder path like Growth/Q3, and the reserved slug Inbox means unfiled. Lists paginate with limit plus an opaque cursor; an empty result is a valid answer, while a lookup by an id that does not exist is an error.";

const _: () = assert!(INSTRUCTIONS.len() < 2048);

/// Handles one parsed JSON-RPC message. Returns the response for a request, or
/// `None` for a notification (no `id`) — including
/// `notifications/initialized`, which is accepted and ignored.
pub fn handle_message(server: &Server, message: Value) -> Option<Value> {
    let Some(object) = message.as_object() else {
        return Some(error_response(Value::Null, RpcError::invalid_request()));
    };

    // A JSON-RPC message with no `id` is a notification: stay silent. The server
    // has no notification side effects.
    if !object.contains_key("id") {
        return None;
    }

    let id = object.get("id").cloned().unwrap_or(Value::Null);
    let method = object.get("method").and_then(Value::as_str);

    let outcome = match method {
        Some("initialize") => Ok(initialize_result()),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(schemas::tools_list().clone()),
        Some("tools/call") => tools::call(server, object.get("params")),
        Some(other) => Err(RpcError::method_not_found(other)),
        None => Err(RpcError::invalid_request()),
    };

    Some(match outcome {
        Ok(result) => success_response(id, result),
        Err(error) => error_response(id, error),
    })
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "kodabi", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use chrono::TimeZone;
    use kodabi_core::index::{IndexedNote, NoteIndex, NoteType};
    use kodabi_core::meeting::{ActionItemFact, MeetingFacts};

    use crate::config::ServerConfig;

    /// A server backed by an in-memory index seeded with meeting notes (with
    /// structured meeting facts), one plain note, and one chat note that also
    /// carries facts, plus a temp vault holding the project folders on disk (for
    /// `list_projects`). The returned `TempDir` must be kept alive for the vault
    /// to exist.
    fn seeded_server() -> (Server, tempfile::TempDir) {
        let (index, vault) = seeded_parts();
        let config = ServerConfig {
            index_db: PathBuf::from("in-memory"),
            kb_root: vault.path().to_path_buf(),
            // The commitment tools each seed their own ledger; the shared
            // server deliberately has none, so every other tool stays covered
            // in the configuration most installs were upgraded from.
            ledger_db: None,
            aging: kodabi_core::ledger::AgingConfig::default(),
        };
        (Server::with_backend(config, index), vault)
    }

    /// The seeded index and its vault, before either is wrapped in a server.
    fn seeded_parts() -> (NoteIndex, tempfile::TempDir) {
        let vault = tempfile::tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();
        fs::create_dir(vault.path().join("Ops")).unwrap();

        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&IndexedNote {
                id: "n_meet01".to_string(),
                path: "Growth/kickoff.md".to_string(),
                title: "Kickoff".to_string(),
                note_type: NoteType::Meeting,
                project: Some("Growth".to_string()),
                date: "2026-07-10".to_string(),
                tags: vec!["planning".to_string()],
                source: "transcript".to_string(),
                confidence: Some(0.9),
                category: None,
                category_confidence: None,
                tracking: None,
                body: "We agreed to ship Phase 3. Action: send the recap.".to_string(),
                meeting: Some(MeetingFacts {
                    duration_seconds: Some(1800),
                    speaker_count: Some(2),
                    decisions: vec!["Ship Phase 3".to_string()],
                    action_items: vec![
                        ActionItemFact {
                            id: "a_recap1".to_string(),
                            description: "send the recap".to_string(),
                            owner: "You".to_string(),
                            // A due date safely in the past → `overdue` regardless
                            // of when the test runs.
                            due_date: Some("2020-01-01".to_string()),
                            done: false,
                            extracted_date: Some("2026-07-10".to_string()),
                        },
                        ActionItemFact {
                            id: "a_room01".to_string(),
                            description: "book the room".to_string(),
                            owner: "Priya".to_string(),
                            due_date: None,
                            done: true,
                            extracted_date: Some("2026-07-10".to_string()),
                        },
                    ],
                }),
            })
            .unwrap();
        index
            .upsert_note(&IndexedNote {
                id: "n_plain1".to_string(),
                path: "Growth/idea.md".to_string(),
                title: "Idea".to_string(),
                note_type: NoteType::Note,
                project: Some("Growth".to_string()),
                date: "2026-07-11".to_string(),
                tags: vec![],
                source: "quick-capture".to_string(),
                confidence: None,
                category: None,
                category_confidence: None,
                tracking: None,
                body: "A standalone thought.".to_string(),
                meeting: None,
            })
            .unwrap();

        // A second meeting whose `source` names a real session transcript on
        // disk, so `get_meeting_transcript` has segments to page. `n_meet01`
        // deliberately keeps the `transcript` keyword, covering the
        // no-transcript-stored branch.
        let transcript_source = seed_session(vault.path());
        index
            .upsert_note(&IndexedNote {
                id: "n_meet02".to_string(),
                path: "Growth/retro.md".to_string(),
                title: "Retro".to_string(),
                note_type: NoteType::Meeting,
                project: Some("Growth".to_string()),
                date: "2026-07-12".to_string(),
                tags: vec![],
                source: transcript_source,
                confidence: None,
                category: None,
                category_confidence: None,
                tracking: None,
                body: "A retro with a transcript.".to_string(),
                meeting: Some(MeetingFacts {
                    duration_seconds: Some(600),
                    speaker_count: Some(2),
                    decisions: vec!["Keep the retro weekly".to_string()],
                    action_items: Vec::new(),
                }),
            })
            .unwrap();

        // A meeting in a sub-project, so `list_outstanding_items` has a subtree
        // to filter on. Its body avoids "recap" so the search ranking tests are
        // unaffected.
        fs::create_dir(vault.path().join("Growth").join("Q3")).unwrap();
        index
            .upsert_note(&IndexedNote {
                id: "n_meet03".to_string(),
                path: "Growth/Q3/planning.md".to_string(),
                title: "Planning".to_string(),
                note_type: NoteType::Meeting,
                project: Some("Growth/Q3".to_string()),
                date: "2026-07-13".to_string(),
                tags: vec![],
                source: "transcript".to_string(),
                confidence: None,
                category: None,
                category_confidence: None,
                tracking: None,
                body: "Quarterly planning.".to_string(),
                meeting: Some(MeetingFacts {
                    duration_seconds: None,
                    speaker_count: None,
                    decisions: vec![],
                    action_items: vec![ActionItemFact {
                        id: "a_plan01".to_string(),
                        description: "draft the plan".to_string(),
                        owner: "Priya".to_string(),
                        // Far enough out to stay `open` however late this runs.
                        due_date: Some("2099-12-31".to_string()),
                        done: false,
                        extracted_date: Some("2026-07-13".to_string()),
                    }],
                }),
            })
            .unwrap();

        // A chat note that carries facts too: its commitments are indexed exactly
        // as a meeting's are. Filed in a sibling project so the `Growth`-scoped
        // tests are untouched by it, and undated so it sorts last among the open
        // items rather than reshuffling the dated ones.
        index
            .upsert_note(&IndexedNote {
                id: "n_chat01".to_string(),
                path: "Ops/greenflow.md".to_string(),
                title: "GreenFlow options".to_string(),
                note_type: NoteType::Chat,
                project: Some("Ops".to_string()),
                date: "2026-07-14".to_string(),
                tags: vec![],
                source: "chats/greenflow.jsonl".to_string(),
                confidence: Some(0.8),
                category: None,
                category_confidence: None,
                tracking: None,
                body: "Talked through the controller options.".to_string(),
                meeting: Some(MeetingFacts {
                    // Always `None` for a chat: there is no recording to measure.
                    duration_seconds: None,
                    speaker_count: None,
                    decisions: vec!["Price the bridge separately".to_string()],
                    action_items: vec![ActionItemFact {
                        id: "a_bridge1".to_string(),
                        description: "ask MERIDIAN for a bridge line item".to_string(),
                        owner: "Jane".to_string(),
                        due_date: None,
                        done: false,
                        extracted_date: Some("2026-07-14".to_string()),
                    }],
                }),
            })
            .unwrap();

        (index, vault)
    }

    /// A server whose vault, index and ledger all describe the *same* two
    /// commitments, as a running app would leave them.
    ///
    /// Real note files on disk, with their action-item ids derived from the
    /// body rather than invented: the write tool re-derives ids from the
    /// Markdown, so a fixture with made-up ids would test nothing. The ledger is
    /// a real file too, since the server opens it by path on every call.
    ///
    /// One of the two commitments is deliberately untracked, so every test can
    /// check that a settled judgement stays out of the default answer.
    fn seeded_server_with_ledger() -> (Server, tempfile::TempDir) {
        seeded_ledger_server(true)
    }

    /// The same fixture with the `KODABI_LEDGER_DB` wiring present or absent, so
    /// the unconfigured case is tested against a vault that really holds the
    /// notes rather than an empty one.
    fn seeded_ledger_server(configured: bool) -> (Server, tempfile::TempDir) {
        use kodabi_core::ledger::{Ledger, UntrackedVia};

        let vault = tempfile::tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();
        fs::create_dir(vault.path().join("Ops")).unwrap();

        let mut index = NoteIndex::open_in_memory().unwrap();
        let ledger_db = vault.path().join("ledger.db");
        let mut ledger = Ledger::open(&ledger_db).unwrap();

        let tracked = seed_tracked_note(
            vault.path(),
            &mut index,
            &mut ledger,
            "n_meet01",
            "Growth",
            "Priya to draft the plan.",
        );
        assert!(!tracked.is_empty(), "the Growth note minted an entry");

        let loose = seed_tracked_note(
            vault.path(),
            &mut index,
            &mut ledger,
            "n_chat01",
            "Ops",
            "Jane to ask MERIDIAN for a bridge line item.",
        );
        // Taken out of the working set by hand: still extracted, no longer tracked.
        ledger
            .untrack(&loose[0], UntrackedVia::Manual, "2026-07-15T00:00:00Z")
            .unwrap();
        drop(ledger);

        let config = ServerConfig {
            index_db: PathBuf::from("in-memory"),
            kb_root: vault.path().to_path_buf(),
            ledger_db: configured.then_some(ledger_db),
            aging: kodabi_core::ledger::AgingConfig::default(),
        };
        (Server::with_backend(config, index), vault)
    }

    /// Writes one note carrying a single action item, indexes it, and syncs the
    /// ledger from the ids the body actually derives. Returns the entry ids the
    /// sync created.
    fn seed_tracked_note(
        vault: &std::path::Path,
        index: &mut NoteIndex,
        ledger: &mut kodabi_core::ledger::Ledger,
        note_id: &str,
        project: &str,
        item_line: &str,
    ) -> Vec<String> {
        use kodabi_core::ledger::{NoteSync, OwnerIdentity};
        use kodabi_core::meeting::derive_meeting_facts;
        use kodabi_core::note::{write_note, Note, NoteId, NoteType, Routing, Source};

        let body = format!("# Summary\n\nWe met.\n\n## Action items\n\n- [ ] {item_line}\n");
        let source = Source::parse("transcript").unwrap();
        let note = Note::new(
            NoteId::parse(note_id).unwrap(),
            NoteType::Meeting,
            Routing::Manual {
                project: project.to_string(),
            },
            "2026-07-10",
            vec![],
            source.clone(),
            &body,
        )
        .unwrap();
        write_note(vault, &note, Some("Seeded note")).unwrap();

        // A meeting body, so the distill grammar splits the owner off the line
        // the way a real distilled note carries it.
        let facts = derive_meeting_facts(
            note_id,
            NoteType::Meeting,
            "2026-07-10",
            &source,
            &body,
            vault,
        );
        index
            .upsert_note(&IndexedNote {
                id: note_id.to_string(),
                path: format!("{project}/seeded-note.md"),
                title: "Seeded note".to_string(),
                note_type: NoteType::Meeting.into(),
                project: Some(project.to_string()),
                date: "2026-07-10".to_string(),
                tags: vec![],
                source: "transcript".to_string(),
                confidence: None,
                category: None,
                category_confidence: None,
                tracking: None,
                body: body.clone(),
                meeting: Some(facts.clone()),
            })
            .unwrap();

        ledger
            .sync_note_items(&NoteSync {
                note_id,
                project,
                note_date_utc: "2026-07-10T00:00:00Z",
                items: &facts.action_items,
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity: &OwnerIdentity::default(),
                now: "2026-07-13T00:00:00Z",
            })
            .unwrap()
            .created
    }

    /// The single action-item id a seeded note derives.
    fn seeded_item_id(server: &Server, note_id: &str) -> String {
        let response = call_tool(server, "get_note", json!({ "id": note_id }));
        response["result"]["structuredContent"]["action_items"][0]["id"]
            .as_str()
            .expect("the seeded note carries one action item")
            .to_string()
    }

    /// The note file's body as it now stands on disk.
    fn read_note_body(vault: &std::path::Path, project: &str) -> String {
        fs::read_to_string(vault.join(project).join("seeded-note.md")).unwrap()
    }

    /// Writes a five-segment session transcript into `<vault>/sessions/` and
    /// returns the `sessions/<file>.jsonl` value a note's `source:` carries.
    fn seed_session(vault: &std::path::Path) -> String {
        use kodabi_core::device::DeviceId;
        use kodabi_core::raw_session::{write_raw_session, TranscriptSegment};
        use kodabi_core::transcription::Channel;

        let segments: Vec<TranscriptSegment> = (0..5)
            .map(|index| TranscriptSegment {
                index,
                channel: if index % 2 == 0 {
                    Channel::You
                } else {
                    Channel::Them
                },
                speaker: None,
                start_ms: index * 1_000,
                end_ms: index * 1_000 + 750,
                text: format!("line {index}"),
            })
            .collect();
        let path = write_raw_session(
            &vault.join("sessions"),
            chrono::Utc.with_ymd_and_hms(2026, 7, 12, 15, 4, 5).unwrap(),
            &DeviceId::parse("k4m2xp7q").unwrap(),
            Some("retro"),
            &segments,
        )
        .unwrap();
        format!("sessions/{}", path.file_name().unwrap().to_str().unwrap())
    }

    fn call_tool(server: &Server, name: &str, arguments: Value) -> Value {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });
        handle_message(server, request).expect("a request draws a response")
    }

    /// Writes a real note file into the vault (the write tools read the vault on
    /// disk, unlike the read tools, which read the in-memory index). An
    /// `Inbox`-routed note is unfiled; any other project is a manual filing.
    fn seed_note(vault: &std::path::Path, id: &str, project: &str) {
        use kodabi_core::note::{write_note, Note, NoteId, NoteType, Routing, Source, Tag, INBOX};
        let routing = if project == INBOX {
            Routing::Routed {
                project: project.to_string(),
                confidence: 0.3,
            }
        } else {
            Routing::Manual {
                project: project.to_string(),
            }
        };
        let note = Note::new(
            NoteId::parse(id).unwrap(),
            NoteType::Note,
            routing,
            "2026-07-10",
            vec![Tag::parse("planning").unwrap()],
            Source::parse("transcript").unwrap(),
            "Body text for the note.",
        )
        .unwrap();
        write_note(vault, &note, Some("Kickoff recap")).unwrap();
    }

    #[test]
    fn initialize_advertises_the_server_identity_and_bounded_instructions() {
        let (server, _vault) = seeded_server();
        let response = handle_message(
            &server,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" }),
        )
        .unwrap();
        let result = &response["result"];
        assert_eq!(result["serverInfo"]["name"], "kodabi");
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["instructions"].as_str().unwrap().len() < 2048);
    }

    #[test]
    fn initialized_notification_draws_no_response() {
        let (server, _vault) = seeded_server();
        let response = handle_message(
            &server,
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        );
        assert!(response.is_none());
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let (server, _vault) = seeded_server();
        let response = handle_message(
            &server,
            json!({ "jsonrpc": "2.0", "id": 3, "method": "frobnicate" }),
        )
        .unwrap();
        assert_eq!(response["error"]["code"], -32601);
    }

    #[test]
    fn tools_list_returns_every_registered_tool() {
        let (server, _vault) = seeded_server();
        let response = handle_message(
            &server,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }),
        )
        .unwrap();
        let names: Vec<&str> = response["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"get_meeting_transcript"), "{names:?}");
        assert!(names.contains(&"search_notes"), "{names:?}");
        assert!(names.contains(&"add_glossary_term"), "{names:?}");
    }

    #[test]
    fn get_note_returns_meeting_metadata_and_action_items() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "get_note", json!({ "id": "n_meet01" }));
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["note"]["id"], "n_meet01");
        assert_eq!(structured["note"]["type"], "meeting");
        assert!(structured["body_markdown"]
            .as_str()
            .unwrap()
            .contains("Phase 3"));

        // `meeting` carries the index-backed MeetingMeta, extending NoteSummary.
        let meeting = &structured["meeting"];
        assert_eq!(meeting["id"], "n_meet01");
        assert_eq!(meeting["type"], "meeting");
        assert_eq!(meeting["duration_seconds"], 1800);
        assert_eq!(meeting["speaker_count"], 2);
        assert_eq!(meeting["decisions"], json!(["Ship Phase 3"]));
        assert_eq!(meeting["action_item_count"], 2);

        // Both action items are returned, in body order, with server-derived
        // status: the open item with a past due date is `overdue`, the checked
        // item is `done`.
        let items = structured["action_items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["id"], "a_recap1");
        assert_eq!(items[0]["owner"], "You");
        assert_eq!(items[0]["status"], "overdue");
        assert_eq!(items[0]["source"]["id"], "n_meet01");
        assert_eq!(items[0]["source"]["path"], "Growth/kickoff.md");
        assert_eq!(items[0]["extracted_date"], "2026-07-10");
        assert_eq!(items[1]["id"], "a_room01");
        assert_eq!(items[1]["status"], "done");
        assert_eq!(items[1]["due_date"], Value::Null);
    }

    #[test]
    fn get_note_returns_null_meeting_and_no_action_items_for_a_plain_note() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "get_note", json!({ "id": "n_plain1" }));
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["note"]["type"], "note");
        assert_eq!(structured["meeting"], Value::Null);
        assert_eq!(structured["action_items"], json!([]));
    }

    /// The two halves of the chat decision, pinned together: action items are
    /// *not* meeting-only, but `meeting` is. `MeetingMeta` leads with
    /// `duration_seconds` + `speaker_count`, which a chat can never populate, so
    /// it stays null rather than being served hollow.
    #[test]
    fn get_note_returns_action_items_but_a_null_meeting_for_a_chat() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "get_note", json!({ "id": "n_chat01" }));
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["note"]["type"], "chat");
        assert_eq!(structured["meeting"], Value::Null);

        let items = structured["action_items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "a_bridge1");
        assert_eq!(items[0]["owner"], "Jane");
        assert_eq!(items[0]["status"], "open");
        assert_eq!(items[0]["source"]["id"], "n_chat01");
    }

    #[test]
    fn get_note_include_action_items_false_omits_the_list_but_keeps_the_count() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_note",
            json!({ "id": "n_meet01", "include_action_items": false }),
        );
        let structured = &response["result"]["structuredContent"];

        // The list is empty, but the meeting metadata still reports the true count.
        assert_eq!(structured["action_items"], json!([]));
        assert_eq!(structured["meeting"]["action_item_count"], 2);
    }

    #[test]
    fn get_note_honors_include_body_false() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_note",
            json!({ "id": "n_meet01", "include_body": false }),
        );
        assert_eq!(
            response["result"]["structuredContent"]["body_markdown"],
            Value::Null
        );
    }

    #[test]
    fn get_note_missing_id_is_a_business_error() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "get_note", json!({ "id": "n_absent" }));
        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response["result"].get("structuredContent").is_none());
        assert!(response.get("error").is_none());
    }

    // --- get_meeting_transcript ---------------------------------------------

    #[test]
    fn get_meeting_transcript_returns_attributed_segments_and_metadata() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_meeting_transcript",
            json!({ "id": "n_meet02" }),
        );
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["note"]["id"], "n_meet02");
        assert_eq!(structured["note"]["path"], "Growth/retro.md");
        assert_eq!(structured["transcript_available"], Value::Bool(true));

        let segments = structured["segments"].as_array().unwrap();
        assert_eq!(segments.len(), 5);
        assert_eq!(segments[0]["index"], 0);
        assert_eq!(segments[0]["channel"], "you");
        assert_eq!(segments[1]["channel"], "them");
        assert_eq!(segments[0]["speaker"], Value::Null);
        assert_eq!(segments[0]["start_ms"], 0);
        assert_eq!(segments[1]["start_ms"], 1_000);
        assert_eq!(segments[0]["text"], "line 0");

        // Metadata rides along by default.
        assert_eq!(structured["meeting"]["duration_seconds"], 600);
        assert_eq!(structured["meeting"]["speaker_count"], 2);
        assert_eq!(
            structured["meeting"]["decisions"][0],
            "Keep the retro weekly"
        );
        assert_eq!(structured["meeting"]["action_item_count"], 0);
        // The whole transcript was read, so the total is exact.
        assert_eq!(structured["page"]["has_more"], Value::Bool(false));
        assert_eq!(structured["page"]["next_cursor"], Value::Null);
        assert_eq!(structured["page"]["total_estimate"], 5);
    }

    #[test]
    fn get_meeting_transcript_include_metadata_false_omits_the_meeting_block() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_meeting_transcript",
            json!({ "id": "n_meet02", "include_metadata": false }),
        );
        let structured = &response["result"]["structuredContent"];

        assert_eq!(structured["meeting"], Value::Null);
        // Segments are unaffected by the metadata toggle.
        assert_eq!(structured["segments"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn get_meeting_transcript_pages_segments_with_a_cursor() {
        let (server, _vault) = seeded_server();

        let mut seen = Vec::new();
        let mut arguments = json!({ "id": "n_meet02", "limit": 2 });
        loop {
            let response = call_tool(&server, "get_meeting_transcript", arguments.clone());
            let structured = &response["result"]["structuredContent"];
            for segment in structured["segments"].as_array().unwrap() {
                seen.push(segment["index"].as_u64().unwrap());
            }
            match structured["page"]["next_cursor"].as_str() {
                Some(cursor) => arguments["cursor"] = json!(cursor),
                None => {
                    assert_eq!(structured["page"]["has_more"], Value::Bool(false));
                    break;
                }
            }
        }

        // Every segment served exactly once, in order.
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn get_meeting_transcript_without_a_stored_transcript_is_not_an_error() {
        let (server, _vault) = seeded_server();
        // `n_meet01`'s source is the `transcript` capture keyword: a real
        // meeting note that never named a session artifact.
        let response = call_tool(
            &server,
            "get_meeting_transcript",
            json!({ "id": "n_meet01" }),
        );
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["transcript_available"], Value::Bool(false));
        assert!(structured["segments"].as_array().unwrap().is_empty());
        assert_eq!(structured["page"]["has_more"], Value::Bool(false));
        // Metadata still answers for a meeting with no transcript.
        assert_eq!(structured["meeting"]["action_item_count"], 2);
    }

    #[test]
    fn get_meeting_transcript_on_a_non_meeting_is_a_business_error() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_meeting_transcript",
            json!({ "id": "n_plain1" }),
        );

        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response["result"].get("structuredContent").is_none());
        assert!(response.get("error").is_none());
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("not a meeting note"), "{text}");
    }

    #[test]
    fn get_meeting_transcript_missing_id_is_a_business_error() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_meeting_transcript",
            json!({ "id": "n_absent" }),
        );

        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response["result"].get("structuredContent").is_none());
        assert!(response.get("error").is_none());
    }

    #[test]
    fn get_meeting_transcript_rejects_a_tampered_cursor() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_meeting_transcript",
            json!({ "id": "n_meet02", "cursor": "not-a-cursor" }),
        );
        assert_eq!(response["error"]["code"], -32602);
    }

    // --- list_outstanding_items ---------------------------------------------

    #[test]
    fn list_outstanding_items_returns_not_done_items_linked_to_their_source() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "list_outstanding_items", json!({}));
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        let items = structured["items"].as_array().unwrap();
        // The overdue 2020 item sorts before the 2099 one, and the undated chat
        // item sorts last; the done item and the undated-but-done one are
        // excluded by the default status set.
        let ids: Vec<&str> = items
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["a_recap1", "a_plan01", "a_bridge1"]);

        assert_eq!(items[0]["status"], "overdue");
        assert_eq!(items[0]["owner"], "You");
        assert_eq!(items[0]["due_date"], "2020-01-01");
        // Each item carries a NoteRef back to the note it came from.
        assert_eq!(items[0]["source"]["id"], "n_meet01");
        assert_eq!(items[0]["source"]["path"], "Growth/kickoff.md");
        assert_eq!(items[1]["status"], "open");

        // The chat-sourced commitment is here on equal footing, pointing back at
        // its chat note. Nothing in this tool filters by note type.
        assert_eq!(items[2]["status"], "open");
        assert_eq!(items[2]["owner"], "Jane");
        assert_eq!(items[2]["due_date"], Value::Null);
        assert_eq!(items[2]["source"]["id"], "n_chat01");
        assert_eq!(items[2]["source"]["path"], "Ops/greenflow.md");

        assert_eq!(structured["summary"]["open"], 2);
        assert_eq!(structured["summary"]["overdue"], 1);
        assert_eq!(structured["summary"]["done"], 0);
        assert_eq!(structured["page"]["has_more"], Value::Bool(false));
        assert_eq!(structured["page"]["total_estimate"], 3);
    }

    #[test]
    fn list_outstanding_items_filters_by_project_subtree() {
        let (server, _vault) = seeded_server();

        // The subtree reaches the sub-project's item.
        let response = call_tool(
            &server,
            "list_outstanding_items",
            json!({ "project": "Growth", "include_descendants": true }),
        );
        let items = response["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap();
        assert_eq!(items.len(), 2);

        // Restricted to the project itself, the nested item drops out.
        let response = call_tool(
            &server,
            "list_outstanding_items",
            json!({ "project": "Growth", "include_descendants": false }),
        );
        let items = response["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "a_recap1");
    }

    #[test]
    fn list_outstanding_items_matches_an_owner_case_insensitively() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "list_outstanding_items", json!({ "owner": "you" }));
        let items = response["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap();

        // Stored as "You"; the caller wrote "you".
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "a_recap1");
    }

    #[test]
    fn list_outstanding_items_empty_match_is_a_successful_empty_page() {
        let (server, _vault) = seeded_server();
        for arguments in [
            json!({ "owner": "nobody-by-that-name" }),
            json!({ "project": "NoSuchProject" }),
            json!({ "source_note_id": "n_absent" }),
        ] {
            let response = call_tool(&server, "list_outstanding_items", arguments.clone());
            let structured = &response["result"]["structuredContent"];

            // A list that matches nothing is a valid answer, never `isError`.
            assert_eq!(
                response["result"]["isError"],
                Value::Bool(false),
                "{arguments}"
            );
            assert!(structured["items"].as_array().unwrap().is_empty());
            assert_eq!(structured["page"]["has_more"], Value::Bool(false));
            assert_eq!(structured["summary"]["open"], 0);
        }
    }

    #[test]
    fn list_outstanding_items_reports_done_only_when_requested() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "list_outstanding_items",
            json!({ "status": ["done"] }),
        );
        let structured = &response["result"]["structuredContent"];
        let items = structured["items"].as_array().unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["id"], "a_room01");
        assert_eq!(items[0]["status"], "done");
        assert_eq!(items[0]["due_date"], Value::Null);
        assert_eq!(structured["summary"]["done"], 1);
        assert_eq!(structured["summary"]["open"], 0);
    }

    #[test]
    fn list_outstanding_items_pages_with_a_cursor() {
        let (server, _vault) = seeded_server();

        let mut seen = Vec::new();
        let mut arguments = json!({ "limit": 1 });
        loop {
            let response = call_tool(&server, "list_outstanding_items", arguments.clone());
            let structured = &response["result"]["structuredContent"];
            for item in structured["items"].as_array().unwrap() {
                seen.push(item["id"].as_str().unwrap().to_string());
            }
            // The totals hold across every page, not just the first.
            assert_eq!(structured["summary"]["open"], 2);
            assert_eq!(structured["summary"]["overdue"], 1);
            match structured["page"]["next_cursor"].as_str() {
                Some(cursor) => arguments["cursor"] = json!(cursor),
                None => break,
            }
        }

        // Three single-item pages spanning both note types, ending on the
        // undated item — the keyset edge, since undated sorts after every date.
        assert_eq!(seen, ["a_recap1", "a_plan01", "a_bridge1"]);
    }

    #[test]
    fn list_outstanding_items_rejects_a_tampered_cursor() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "list_outstanding_items",
            json!({ "cursor": "not-a-cursor" }),
        );
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn list_outstanding_items_rejects_an_unknown_argument() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "list_outstanding_items",
            json!({ "assignee": "You" }),
        );
        assert_eq!(response["error"]["code"], -32602);
    }

    // --- get_project_context ------------------------------------------------

    /// Adds a README and a glossary to `Growth`, for the briefing tests.
    fn seed_project_files(vault: &std::path::Path) {
        use kodabi_core::glossary::{Glossary, GlossaryTerm, OnConflict};

        let project_dir = vault.join("Growth");
        fs::write(
            project_dir.join("README.md"),
            "# Growth\n\nThe growth workstream.\n",
        )
        .unwrap();

        let mut glossary = Glossary::default();
        glossary
            .upsert(
                GlossaryTerm {
                    term: "MERIDIAN".to_string(),
                    definition: "A systems-migration project.".to_string(),
                    aliases: vec!["meridian".to_string()],
                },
                OnConflict::Update,
            )
            .unwrap();
        glossary.save(&project_dir).unwrap();
    }

    #[test]
    fn get_project_context_assembles_every_section_and_its_counts() {
        let (server, vault) = seeded_server();
        seed_project_files(vault.path());

        let response = call_tool(
            &server,
            "get_project_context",
            json!({ "project": "Growth" }),
        );
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["project"]["slug"], "Growth");
        assert_eq!(structured["project"]["display_name"], "Growth");
        assert_eq!(structured["project"]["parent"], Value::Null);
        assert_eq!(
            structured["description"],
            "# Growth\n\nThe growth workstream."
        );

        // The glossary carries the project the `_glossary.yml` file leaves implicit.
        let glossary = structured["glossary"].as_array().unwrap();
        assert_eq!(glossary.len(), 1);
        assert_eq!(glossary[0]["term"], "MERIDIAN");
        assert_eq!(glossary[0]["project"], "Growth");

        // Descendants are off by default: the three `Growth` notes, not the
        // `Growth/Q3` one.
        assert_eq!(structured["counts"]["notes"], 3);
        assert_eq!(structured["counts"]["meetings"], 2);
        assert_eq!(structured["counts"]["notes_by_type"]["meeting"], 2);
        assert_eq!(structured["counts"]["notes_by_type"]["note"], 1);
        assert_eq!(structured["counts"]["notes_by_type"]["chat"], 0);
        assert_eq!(structured["counts"]["glossary_terms"], 1);
        assert_eq!(structured["counts"]["outstanding_overdue"], 1);
        assert_eq!(structured["counts"]["outstanding_open"], 0);

        let outstanding = structured["outstanding"].as_array().unwrap();
        assert_eq!(outstanding.len(), 1);
        assert_eq!(outstanding[0]["id"], "a_recap1");

        assert_eq!(structured["recent_notes"].as_array().unwrap().len(), 3);
        // The one documented pagination exception: no cursor at all.
        assert!(structured.get("page").is_none());
    }

    #[test]
    fn get_project_context_include_descendants_widens_the_scope() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_project_context",
            json!({ "project": "Growth", "include_descendants": true }),
        );
        let counts = &response["result"]["structuredContent"]["counts"];

        // The `Growth/Q3` meeting and its open item join in.
        assert_eq!(counts["notes"], 4);
        assert_eq!(counts["meetings"], 3);
        assert_eq!(counts["outstanding_open"], 1);
        assert_eq!(counts["outstanding_overdue"], 1);
    }

    #[test]
    fn get_project_context_keeps_counts_true_when_sections_are_capped() {
        let (server, vault) = seeded_server();
        seed_project_files(vault.path());

        let response = call_tool(
            &server,
            "get_project_context",
            json!({
                "project": "Growth",
                "recent_notes_limit": 0,
                "outstanding_limit": 0,
                "include_glossary": false
            }),
        );
        let structured = &response["result"]["structuredContent"];

        assert!(structured["recent_notes"].as_array().unwrap().is_empty());
        assert!(structured["outstanding"].as_array().unwrap().is_empty());
        assert!(structured["glossary"].as_array().unwrap().is_empty());

        // A caller that drops a section still learns how much is there.
        assert_eq!(structured["counts"]["notes"], 3);
        assert_eq!(structured["counts"]["outstanding_overdue"], 1);
        assert_eq!(structured["counts"]["glossary_terms"], 1);
    }

    #[test]
    fn get_project_context_without_a_readme_reports_a_null_description() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_project_context",
            json!({ "project": "Growth" }),
        );
        assert_eq!(
            response["result"]["structuredContent"]["description"],
            Value::Null
        );
    }

    #[test]
    fn get_project_context_unknown_project_is_a_business_error() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_project_context",
            json!({ "project": "NoSuchProject" }),
        );

        // A lookup by a slug that misses is an error: the caller asserted it
        // exists (unlike a list, where empty is a valid answer).
        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response["result"].get("structuredContent").is_none());
        assert!(response.get("error").is_none());
    }

    #[test]
    fn get_project_context_rejects_a_malformed_slug() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "get_project_context",
            json!({ "project": "Inbox/nope" }),
        );
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn search_notes_returns_ranked_hits() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "search_notes", json!({ "query": "recap" }));
        let hits = response["result"]["structuredContent"]["hits"]
            .as_array()
            .unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0]["id"], "n_meet01");
        assert_eq!(hits[0]["type"], "meeting");
    }

    #[test]
    fn search_notes_empty_match_is_a_successful_empty_page() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "search_notes",
            json!({ "query": "nothingmatchesthisword" }),
        );
        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(response["result"]["structuredContent"]["hits"], json!([]));
        assert_eq!(
            response["result"]["structuredContent"]["page"]["has_more"],
            Value::Bool(false)
        );
    }

    #[test]
    fn list_projects_enumerates_disk_projects() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "list_projects", json!({}));
        let projects = response["result"]["structuredContent"]["projects"]
            .as_array()
            .unwrap();
        assert!(projects.iter().any(|p| p["slug"] == "Growth"));
    }

    #[test]
    fn unknown_tool_is_invalid_params() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "no_such_tool", json!({}));
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn malformed_arguments_are_invalid_params() {
        let (server, _vault) = seeded_server();
        // `get_note` requires `id`; omitting it fails deserialization.
        let response = call_tool(&server, "get_note", json!({}));
        assert_eq!(response["error"]["code"], -32602);
    }

    // --- file_note_to_project ---------------------------------------------

    #[test]
    fn file_note_to_project_reroutes_an_inbox_note() {
        let (server, vault) = seeded_server();
        seed_note(vault.path(), "n_route1", "Inbox");

        let response = call_tool(
            &server,
            "file_note_to_project",
            json!({ "id": "n_route1", "project": "Growth" }),
        );
        let structured = &response["result"]["structuredContent"];

        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["moved"], Value::Bool(true));
        assert_eq!(structured["note"]["id"], "n_route1");
        assert_eq!(structured["note"]["project"], "Growth");
        // Omitted confidence records a human correction as 1.0.
        assert_eq!(structured["note"]["confidence"], json!(1.0));
        assert!(structured["note"]["path"]
            .as_str()
            .unwrap()
            .starts_with("Growth/"));
        // Re-routed from the Inbox, so the previous project is null.
        assert_eq!(structured["previous"]["project"], Value::Null);
    }

    #[test]
    fn file_note_to_project_unknown_id_is_a_business_error() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "file_note_to_project",
            json!({ "id": "n_absent", "project": "Growth" }),
        );
        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response["result"].get("structuredContent").is_none());
        assert!(response.get("error").is_none());
    }

    #[test]
    fn file_note_to_project_missing_target_project_is_a_business_error() {
        let (server, vault) = seeded_server();
        seed_note(vault.path(), "n_route2", "Inbox");

        let response = call_tool(
            &server,
            "file_note_to_project",
            json!({ "id": "n_route2", "project": "Nonexistent" }),
        );
        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response.get("error").is_none());
    }

    #[test]
    fn file_note_to_project_create_project_files_into_a_new_folder() {
        let (server, vault) = seeded_server();
        seed_note(vault.path(), "n_route3", "Inbox");

        let response = call_tool(
            &server,
            "file_note_to_project",
            json!({ "id": "n_route3", "project": "NewProj", "create_project": true }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["moved"], Value::Bool(true));
        assert_eq!(structured["note"]["project"], "NewProj");
        assert!(vault.path().join("NewProj").is_dir());
    }

    #[test]
    fn file_note_to_project_invalid_id_is_invalid_params() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "file_note_to_project",
            json!({ "id": "not-a-note-id", "project": "Growth" }),
        );
        assert_eq!(response["error"]["code"], -32602);
    }

    // --- add_glossary_term ------------------------------------------------

    #[test]
    fn add_glossary_term_creates_a_term() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "add_glossary_term",
            json!({
                "project": "Growth",
                "term": "MERIDIAN",
                "definition": "A systems-migration project.",
                "aliases": ["meridian"]
            }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["created"], Value::Bool(true));
        assert_eq!(structured["term"]["term"], "MERIDIAN");
        assert_eq!(structured["term"]["project"], "Growth");
        assert_eq!(structured["term"]["aliases"], json!(["meridian"]));
    }

    #[test]
    fn add_glossary_term_updates_an_existing_term() {
        let (server, _vault) = seeded_server();
        call_tool(
            &server,
            "add_glossary_term",
            json!({ "project": "Growth", "term": "MERIDIAN", "definition": "First." }),
        );
        let response = call_tool(
            &server,
            "add_glossary_term",
            json!({ "project": "Growth", "term": "meridian", "definition": "Second." }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], Value::Bool(false));
        assert_eq!(structured["created"], Value::Bool(false));
        assert_eq!(structured["term"]["definition"], "Second.");
    }

    #[test]
    fn add_glossary_term_on_conflict_error_is_a_business_error() {
        let (server, _vault) = seeded_server();
        call_tool(
            &server,
            "add_glossary_term",
            json!({ "project": "Growth", "term": "MERIDIAN", "definition": "First." }),
        );
        let response = call_tool(
            &server,
            "add_glossary_term",
            json!({
                "project": "Growth",
                "term": "MERIDIAN",
                "definition": "Second.",
                "on_conflict": "error"
            }),
        );
        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response["result"].get("structuredContent").is_none());
        assert!(response.get("error").is_none());
    }

    #[test]
    fn add_glossary_term_missing_project_is_a_business_error() {
        let (server, _vault) = seeded_server();
        let response = call_tool(
            &server,
            "add_glossary_term",
            json!({ "project": "Nonexistent", "term": "X", "definition": "Y." }),
        );
        assert_eq!(response["result"]["isError"], Value::Bool(true));
        assert!(response.get("error").is_none());
    }
    // ----- list_commitments -------------------------------------------------

    #[test]
    fn list_commitments_serves_the_tracked_working_set() {
        let (server, _vault) = seeded_server_with_ledger();
        let response = call_tool(&server, "list_commitments", json!({}));
        let structured = &response["result"]["structuredContent"];

        let rows = structured["commitments"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            1,
            "the untracked entry is not in the working set"
        );
        let row = &rows[0];
        assert_eq!(row["state"], "open");
        assert_eq!(row["description"], "draft the plan");
        assert_eq!(row["project"], "Growth");
        // The live source line is joined in from the index, under the id the
        // note body derives.
        assert_eq!(row["item"]["item_id"], seeded_item_id(&server, "n_meet01"));
        assert_eq!(row["item"]["done"], false);
        assert_eq!(row["source"]["note_id"], "n_meet01");
        assert_eq!(structured["summary"]["total"], 1);
        assert_eq!(structured["page"]["has_more"], false);
    }

    /// The reason this tool exists: raw extraction and the ledger disagree, and
    /// each tool answers for its own store.
    #[test]
    fn an_untracked_item_is_outstanding_but_not_a_commitment() {
        let (server, _vault) = seeded_server_with_ledger();

        let outstanding = call_tool(
            &server,
            "list_outstanding_items",
            json!({ "project": "Ops" }),
        );
        let items = outstanding["result"]["structuredContent"]["items"]
            .as_array()
            .unwrap();
        assert_eq!(items.len(), 1, "extraction still reports the line");
        assert_eq!(items[0]["id"], seeded_item_id(&server, "n_chat01"));

        let commitments = call_tool(&server, "list_commitments", json!({ "project": "Ops" }));
        let rows = commitments["result"]["structuredContent"]["commitments"]
            .as_array()
            .unwrap();
        assert!(rows.is_empty(), "the ledger does not track it");

        // Asking for it by state finds it, so nothing is hidden — only defaulted away.
        let settled = call_tool(
            &server,
            "list_commitments",
            json!({ "project": "Ops", "state": ["untracked"] }),
        );
        let rows = settled["result"]["structuredContent"]["commitments"]
            .as_array()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["untracked_via"], "manual");
    }

    #[test]
    fn an_unknown_project_is_an_empty_commitments_page_not_an_error() {
        let (server, _vault) = seeded_server_with_ledger();
        let response = call_tool(&server, "list_commitments", json!({ "project": "Nope" }));
        let structured = &response["result"]["structuredContent"];
        assert!(structured["commitments"].as_array().unwrap().is_empty());
        assert_eq!(structured["page"]["has_more"], false);
        assert_eq!(response["result"]["isError"], false);
    }

    #[test]
    fn a_bad_commitments_cursor_is_invalid_params() {
        let (server, _vault) = seeded_server_with_ledger();
        let response = call_tool(&server, "list_commitments", json!({ "cursor": "nope" }));
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn an_unknown_commitments_argument_is_rejected() {
        let (server, _vault) = seeded_server_with_ledger();
        let response = call_tool(&server, "list_commitments", json!({ "projects": "Growth" }));
        assert_eq!(response["error"]["code"], -32602);
    }

    /// A `.mcp.json` written before these tools existed names no ledger. The
    /// error says which variable to set rather than inventing a path.
    #[test]
    fn commitments_without_a_configured_ledger_name_the_variable() {
        let (server, _vault) = seeded_server();
        let response = call_tool(&server, "list_commitments", json!({}));
        assert_eq!(response["error"]["code"], -32603);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("KODABI_LEDGER_DB"));
    }

    #[test]
    fn a_project_briefing_carries_the_ledgers_counts_when_one_is_configured() {
        let (with_ledger, _vault) = seeded_server_with_ledger();
        let response = call_tool(
            &with_ledger,
            "get_project_context",
            json!({ "project": "Growth" }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["commitments"]["total"], 1);
        // Raw extraction is still reported separately, and still counts.
        assert!(structured["counts"]["outstanding_open"].as_u64().unwrap() >= 1);

        // Without a ledger the block is null: the briefing cannot speak to
        // tracking, which is not the same as nothing being tracked.
        let (without, _vault) = seeded_server();
        let response = call_tool(
            &without,
            "get_project_context",
            json!({ "project": "Growth" }),
        );
        assert!(response["result"]["structuredContent"]["commitments"].is_null());
    }

    // ----- update_action_item -----------------------------------------------

    /// The write both halves: the checkbox in the note, and the judgement in the
    /// ledger. Either alone is the disagreement this tool exists to prevent.
    #[test]
    fn marking_an_item_done_ticks_the_note_and_closes_the_commitment() {
        let (server, vault) = seeded_server_with_ledger();
        let item = seeded_item_id(&server, "n_meet01");

        let response = call_tool(
            &server,
            "update_action_item",
            json!({ "note_id": "n_meet01", "item_id": item, "done": true }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(structured["note_outcome"], "updated");
        assert_eq!(structured["note_updated"], true);
        assert_eq!(structured["entry"]["state"], "closed");
        // A chat-made close is a person's own judgement, like one made in the app.
        assert_eq!(structured["entry"]["closed_via"], "manual");

        // The note on disk really carries the tick.
        let body = read_note_body(vault.path(), "Growth");
        assert!(body.contains("- [x]"), "the checkbox was written: {body}");

        // And the commitment has left the working set.
        let listed = call_tool(&server, "list_commitments", json!({}));
        assert!(listed["result"]["structuredContent"]["commitments"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reopening_unticks_the_note_and_reopens_the_commitment() {
        let (server, vault) = seeded_server_with_ledger();
        let item = seeded_item_id(&server, "n_meet01");

        let done = json!({ "note_id": "n_meet01", "item_id": item, "done": true });
        call_tool(&server, "update_action_item", done);

        let response = call_tool(
            &server,
            "update_action_item",
            json!({ "note_id": "n_meet01", "item_id": item, "done": false }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(structured["entry"]["state"], "open");
        assert!(structured["entry"]["closed_via"].is_null());
        let body = read_note_body(vault.path(), "Growth");
        assert!(body.contains("- [ ]"), "the checkbox was cleared: {body}");
    }

    /// Asking twice is a success, not an error: the person got what they wanted.
    #[test]
    fn marking_an_already_done_item_done_again_succeeds() {
        let (server, _vault) = seeded_server_with_ledger();
        let item = seeded_item_id(&server, "n_meet01");
        let args = json!({ "note_id": "n_meet01", "item_id": item, "done": true });

        call_tool(&server, "update_action_item", args.clone());
        let response = call_tool(&server, "update_action_item", args);
        let structured = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(structured["note_outcome"], "already_set");
        assert_eq!(structured["note_updated"], false);
        assert_eq!(structured["entry"]["state"], "closed");
    }

    /// The checkbox is the source of truth for done/not-done, so an item the
    /// person took out of the working set still ticks — and the ledger is left
    /// exactly as they set it rather than being argued with.
    #[test]
    fn an_untracked_item_ticks_without_moving_the_ledger() {
        let (server, vault) = seeded_server_with_ledger();
        let item = seeded_item_id(&server, "n_chat01");

        let response = call_tool(
            &server,
            "update_action_item",
            json!({ "note_id": "n_chat01", "item_id": item, "done": true }),
        );
        let structured = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], false);
        assert_eq!(structured["note_outcome"], "updated");
        assert!(read_note_body(vault.path(), "Ops").contains("- [x]"));
        assert_eq!(
            structured["entry"]["state"], "untracked",
            "reported as-is, with no judgement invented for it"
        );
    }

    #[test]
    fn updating_an_unknown_note_is_a_business_error() {
        let (server, _vault) = seeded_server_with_ledger();
        let response = call_tool(
            &server,
            "update_action_item",
            json!({ "note_id": "n_zzzzzz", "item_id": "a_whatever", "done": true }),
        );
        assert_eq!(response["result"]["isError"], true);
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("note not found"));
    }

    #[test]
    fn a_malformed_note_id_is_invalid_params() {
        let (server, _vault) = seeded_server_with_ledger();
        let response = call_tool(
            &server,
            "update_action_item",
            json!({ "note_id": "not-an-id", "item_id": "a_whatever", "done": true }),
        );
        assert_eq!(response["error"]["code"], -32602);
    }

    /// A forbidden transition is something the model can act on (reopen first),
    /// so it comes back as a business answer naming both states.
    #[test]
    fn closing_a_waived_commitment_is_refused_in_the_callers_terms() {
        let (server, vault) = seeded_server_with_ledger();
        let item = seeded_item_id(&server, "n_meet01");

        {
            use kodabi_core::ledger::{EntryFilter, Ledger};
            let mut ledger = Ledger::open(&vault.path().join("ledger.db")).unwrap();
            let open = ledger
                .list_entries(&EntryFilter {
                    states: Some(vec![kodabi_core::ledger::EntryState::Open]),
                    project: None,
                    include_descendants: false,
                    direction: None,
                    note_id: None,
                    stale_before: None,
                    unchecked_since: None,
                })
                .unwrap();
            ledger
                .waive(&open[0].entry_id, "2026-07-16T00:00:00Z")
                .unwrap();
        }

        let response = call_tool(
            &server,
            "update_action_item",
            json!({ "note_id": "n_meet01", "item_id": item, "done": true }),
        );
        assert_eq!(response["result"]["isError"], true);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("waived"), "names where it is: {text}");
        assert!(text.contains("closed"), "names where it would go: {text}");
    }

    /// A configuration fault must never leave a ticked box with nothing recorded
    /// against it, so the refusal happens before the vault is touched.
    #[test]
    fn updating_without_a_configured_ledger_leaves_the_note_untouched() {
        let (server, vault) = seeded_ledger_server(false);
        let item = seeded_item_id(&server, "n_meet01");
        let before = read_note_body(vault.path(), "Growth");

        let response = call_tool(
            &server,
            "update_action_item",
            json!({ "note_id": "n_meet01", "item_id": item, "done": true }),
        );
        assert_eq!(response["error"]["code"], -32603);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("KODABI_LEDGER_DB"));
        assert_eq!(
            before,
            read_note_body(vault.path(), "Growth"),
            "nothing was written"
        );
    }
}
