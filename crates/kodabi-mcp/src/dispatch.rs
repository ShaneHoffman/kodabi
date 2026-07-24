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
const INSTRUCTIONS: &str = "Kodabi read tools over your local knowledge base of distilled, routed Markdown notes. Use search_notes for hybrid full-text and semantic retrieval (ranked snippets), get_note to read a hit in full by its stable id, and list_projects to resolve a project name to its slug before filtering. Ids and slugs are the handles: a note id looks like n_a1b2c3, a project slug is a folder path like Growth/Q3, and the reserved slug Inbox means unfiled. Lists paginate with limit plus an opaque cursor; an empty result is a valid answer, while a lookup by an id that does not exist is an error.";

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

    use kodabi_core::index::{IndexedNote, NoteIndex, NoteType};
    use kodabi_core::meeting::{ActionItemFact, MeetingFacts};

    use crate::config::ServerConfig;

    /// A server backed by an in-memory index seeded with one meeting note (with
    /// structured meeting facts) and one plain note, plus a temp vault holding one
    /// project folder on disk (for `list_projects`). The returned `TempDir` must
    /// be kept alive for the vault to exist.
    fn seeded_server() -> (Server, tempfile::TempDir) {
        let vault = tempfile::tempdir().unwrap();
        fs::create_dir(vault.path().join("Growth")).unwrap();

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
                body: "A standalone thought.".to_string(),
                meeting: None,
            })
            .unwrap();

        let config = ServerConfig {
            index_db: PathBuf::from("in-memory"),
            kb_root: vault.path().to_path_buf(),
        };
        (Server::with_backend(config, index), vault)
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
    fn tools_list_returns_the_three_tools() {
        let (server, _vault) = seeded_server();
        let response = handle_message(
            &server,
            json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }),
        )
        .unwrap();
        assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 3);
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
}
