//! End-to-end check of the built binary: a real stdio handshake, a `tools/call`,
//! and — critically — that stdout carries only JSON-RPC frames (stderr-only
//! diagnostics discipline).

use std::io::Write;
use std::process::{Command, Stdio};

use kodabi_core::index::{IndexedNote, NoteIndex, NoteType};

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
                note_type: NoteType::Note,
                project: Some("Growth".to_string()),
                date: "2026-07-10".to_string(),
                tags: vec![],
                source: "manual".to_string(),
                confidence: None,
                body: "The quarterly plan.".to_string(),
            })
            .unwrap();
    }

    let mut child = Command::new(env!("CARGO_BIN_EXE_kodabi-mcp"))
        .env("KODABI_INDEX_DB", &index_path)
        .env("KODABI_KB_ROOT", &vault)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let requests = concat!(
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
        "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"list_projects\",\"arguments\":{}}}\n",
        "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"add_glossary_term\",\"arguments\":{\"project\":\"Growth\",\"term\":\"MERIDIAN\",\"definition\":\"A migration project.\"}}}\n",
    );
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
    // Stdout purity: every non-empty line must parse as a JSON-RPC frame.
    let responses: Vec<serde_json::Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("non-JSON on stdout: {line:?} ({error})"))
        })
        .collect();

    // Three responses: initialize (id 1), list_projects (id 2), and
    // add_glossary_term (id 3). The notification draws none.
    assert_eq!(responses.len(), 3);

    let init = responses.iter().find(|r| r["id"] == 1).unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "kodabi");
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    let list = responses.iter().find(|r| r["id"] == 2).unwrap();
    let projects = list["result"]["structuredContent"]["projects"]
        .as_array()
        .unwrap();
    assert!(projects.iter().any(|project| project["slug"] == "Growth"));

    // The write tool round-trips over stdio: a new glossary term is created.
    let added = responses.iter().find(|r| r["id"] == 3).unwrap();
    assert_eq!(added["result"]["isError"], serde_json::Value::Bool(false));
    assert_eq!(added["result"]["structuredContent"]["created"], true);
    assert_eq!(
        added["result"]["structuredContent"]["term"]["term"],
        "MERIDIAN"
    );

    // The write landed on disk in the project's glossary file.
    let glossary = std::fs::read_to_string(vault.join("Growth").join("_glossary.yml")).unwrap();
    assert!(glossary.contains("MERIDIAN"));
}
