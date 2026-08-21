//! Integration tests that drive a real bidirectional stream-json `claude`
//! through [`kodabi_llm::chat::spawn_chat`], proving the two load-bearing
//! properties of the chat seam: multi-turn memory in one long-lived process,
//! and the `can_use_tool` permission round-trip over stdin/stdout.
//!
//! `#[ignore]` because they spend a small amount of real Claude usage and
//! require a working, authenticated `claude` CLI on `PATH`. The permission
//! test additionally needs the `kodabi-mcp` binary built
//! (`cargo build -p kodabi-mcp`). Run with:
//!
//! ```text
//! cargo test -p kodabi-llm --test chat_real -- --ignored
//! ```

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use kodabi_core::chat::{user_message_json, ChatStreamItem, PermissionDecision};
use kodabi_core::terminal::{mcp_config_json, McpPaths};
use kodabi_llm::chat::{spawn_chat, ChatChild, ChatChildEvent, ChatProcessConfig};

const TURN_DEADLINE: Duration = Duration::from_secs(120);

/// Collects events until the end of the current turn (a `TurnResult`),
/// panicking if the process exits or the deadline passes first.
fn collect_turn(events: &Receiver<ChatChildEvent>) -> Vec<ChatStreamItem> {
    let deadline = Instant::now() + TURN_DEADLINE;
    let mut items = Vec::new();
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("turn should complete within the deadline");
        match events
            .recv_timeout(remaining)
            .expect("chat process should keep streaming until the turn ends")
        {
            ChatChildEvent::Item(item) => {
                let done = matches!(item, ChatStreamItem::TurnResult { .. });
                items.push(item);
                if done {
                    return items;
                }
            }
            ChatChildEvent::Exited { code } => {
                panic!("chat process exited mid-turn (code {code:?}); items so far: {items:?}")
            }
        }
    }
}

fn assistant_text(items: &[ChatStreamItem]) -> String {
    items
        .iter()
        .filter_map(|item| match item {
            ChatStreamItem::AssistantText { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn cheap_config() -> ChatProcessConfig {
    ChatProcessConfig {
        model: "haiku".to_owned(),
        ..ChatProcessConfig::from_env()
    }
}

fn session_id() -> String {
    // A fixed-shape v4-style UUID derived from the pid keeps runs distinct
    // without pulling a uuid dependency into dev-deps.
    format!("00000000-0000-4000-8000-{:012x}", std::process::id())
}

#[test]
#[ignore = "spawns a real headless `claude` process and spends a small amount of real usage"]
fn one_process_carries_a_multi_turn_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let mcp_config = dir.path().join("empty.mcp.json");
    std::fs::write(&mcp_config, r#"{"mcpServers":{}}"#).unwrap();

    let (child, events) =
        spawn_chat(&cheap_config(), &mcp_config, &session_id(), dir.path()).unwrap();

    child
        .write_line(user_message_json("Reply with exactly the word: pong"))
        .unwrap();
    let first = collect_turn(&events);
    assert!(
        assistant_text(&first).contains("pong"),
        "first turn should answer pong, got: {first:?}"
    );

    child
        .write_line(user_message_json(
            "Reply with exactly the word you replied with before, in uppercase.",
        ))
        .unwrap();
    let second = collect_turn(&events);
    assert!(
        assistant_text(&second).contains("PONG"),
        "second turn should remember the first (PONG), got: {second:?}"
    );

    child.kill();
}

#[test]
#[ignore = "spawns real `claude` + `kodabi-mcp` processes and spends a small amount of real usage"]
fn a_write_tool_prompts_over_the_control_protocol_and_deny_is_honored() {
    let mcp_binary = {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.pop(); // crates/
        path.pop(); // workspace root
        path.push("target");
        path.push("debug");
        path.push(if cfg!(windows) {
            "kodabi-mcp.exe"
        } else {
            "kodabi-mcp"
        });
        assert!(
            path.is_file(),
            "kodabi-mcp not built; run `cargo build -p kodabi-mcp` first ({path:?})"
        );
        path
    };

    let kb = tempfile::tempdir().unwrap();
    std::fs::create_dir(kb.path().join("Briarwood Golf")).unwrap();
    let mcp_config = kb.path().join("kodabi.mcp.json");
    let config_body = mcp_config_json(&McpPaths {
        mcp_binary,
        index_db: kb.path().join("index.db"),
        kb_root: kb.path().to_path_buf(),
        ledger_db: kb.path().join("ledger.db"),
        aging: Default::default(),
    })
    .unwrap();
    std::fs::write(&mcp_config, config_body).unwrap();

    let (child, events) =
        spawn_chat(&cheap_config(), &mcp_config, &session_id(), kb.path()).unwrap();

    child
        .write_line(user_message_json(
            "Use the mcp__kodabi__add_glossary_term tool to add the term 'AEC' with \
             definition 'acoustic echo cancellation' to the project 'Briarwood Golf'. \
             Call the tool directly, do not ask me first.",
        ))
        .unwrap();

    // The turn must pause on a can_use_tool request for the write tool.
    let deadline = Instant::now() + TURN_DEADLINE;
    let request_id = loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("permission request should arrive within the deadline");
        match events
            .recv_timeout(remaining)
            .expect("stream should continue")
        {
            ChatChildEvent::Item(ChatStreamItem::PermissionRequest {
                request_id,
                tool_name,
                ..
            }) => {
                assert_eq!(tool_name, "mcp__kodabi__add_glossary_term");
                break request_id;
            }
            ChatChildEvent::Item(ChatStreamItem::TurnResult { .. }) => {
                panic!("turn ended without a permission request")
            }
            ChatChildEvent::Exited { code } => panic!("chat process exited (code {code:?})"),
            _ => {}
        }
    };

    deny(&child, &request_id);
    let rest = collect_turn(&events);
    assert!(
        rest.iter()
            .any(|item| matches!(item, ChatStreamItem::TurnResult { .. })),
        "turn should complete after the deny"
    );

    // Denied means the write never happened.
    assert!(
        !kb.path()
            .join("Briarwood Golf")
            .join("_glossary.yml")
            .exists(),
        "deny must prevent the glossary write"
    );

    child.kill();
}

fn deny(child: &ChatChild, request_id: &str) {
    child
        .write_line(kodabi_core::chat::permission_response_json(
            request_id,
            &PermissionDecision::Deny {
                message: "User declined in Kodabi".to_owned(),
            },
        ))
        .unwrap();
}
