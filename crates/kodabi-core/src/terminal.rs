//! Pure builders and the byte pump for the embedded Claude Code terminal.
//!
//! The terminal view (Phase 3, FOUNDING_DOC §4) hosts an *interactive* `claude`
//! process wired to the `kodabi` MCP server, so chat-over-the-knowledge-base
//! works with zero setup. The side-effecting PTY spawn and the machine-path
//! resolution live in the Tauri shell (`src-tauri/src/terminal_cmds.rs`);
//! everything here is pure and unit-tested without a subprocess — the argv, the
//! generated `.mcp.json` and Claude Code settings, the resize validation, and
//! the reader/coalescer loops that turn PTY bytes into output batches. Those
//! loops follow [`crate::watch::watch_vault`]'s split: parameterized on the
//! channel, the timings and the sinks, so tests drive them with synthetic
//! chunks and no PTY. This mirrors how `llm`'s pure request/parse logic stays
//! testable apart from `kodabi-llm`'s process spawn.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

/// The MCP server name, so its tools are namespaced `mcp__kodabi__<tool>`
/// (`docs/MCP_TOOL_SURFACE.md` §Server & wiring).
pub const MCP_SERVER_KEY: &str = "kodabi";

/// The read tools, pre-approved so chat-over-the-KB needs no per-tool
/// permission prompt. The three write tools (`file_note_to_project`,
/// `add_glossary_term`, `update_action_item`) are deliberately omitted, so
/// Claude Code still prompts for them — there is a real TTY in the terminal to
/// answer.
///
/// Must list every `read_only` entry of `crates/kodabi-mcp/src/schemas.rs`'s
/// `TOOLS` table, **in that table's order**: a read tool missing here still
/// works, but prompts on every call, which is exactly the friction the embedded
/// terminal exists to remove. The parity test lives in `schemas.rs` (kodabi-core
/// cannot see kodabi-mcp) and compares the whole list, so a new read tool is
/// inserted at its table position rather than appended.
pub const READ_TOOL_PERMISSIONS: [&str; 7] = [
    "mcp__kodabi__search_notes",
    "mcp__kodabi__get_note",
    "mcp__kodabi__get_meeting_transcript",
    "mcp__kodabi__list_outstanding_items",
    "mcp__kodabi__list_commitments",
    "mcp__kodabi__list_projects",
    "mcp__kodabi__get_project_context",
];

/// Set on the spawned `claude` process to disable its own transcript and
/// prompt-history writing (`~/.claude/projects/…`), so the in-app retention
/// promise holds for chat too (FOUNDING_DOC §3.7, the Phase 2 leftover). Scoped
/// to Kodabi's launch — a per-process env var, so it never touches the user's
/// global Claude Code config or their other sessions.
pub const SKIP_HISTORY_ENV: &str = "CLAUDE_CODE_SKIP_PROMPT_HISTORY";

/// The largest terminal grid we forward to the PTY. A resize can arrive with a
/// wild value from a detached or zero-size container; clamp rather than trust.
const MAX_DIMENSION: u16 = 1000;

/// Resolved, machine-local absolute paths the shell computes and hands in,
/// plus the few settings the server cannot read for itself.
#[derive(Debug, Clone)]
pub struct McpPaths {
    /// Absolute path to the `kodabi-mcp` binary.
    pub mcp_binary: PathBuf,
    /// The SQLite note index (`KODABI_INDEX_DB`).
    pub index_db: PathBuf,
    /// The knowledge-base (vault) root (`KODABI_KB_ROOT`).
    pub kb_root: PathBuf,
    /// The commitment ledger (`KODABI_LEDGER_DB`).
    ///
    /// The server reads it for `list_commitments` and writes it for
    /// `update_action_item`. It lives in the *config* dir rather than beside the
    /// index, so neither of the other two variables locates it.
    pub ledger_db: PathBuf,
    /// The user's aging thresholds, which live in `settings.toml` — a file the
    /// server has no path to and no business parsing. Passed through so a
    /// commitment's tier reads the same in chat as it does in the Commitments
    /// view; the server falls back to the same defaults if they are absent.
    pub aging: crate::ledger::AgingConfig,
}

#[derive(Serialize)]
struct McpServerEntry {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct McpConfigFile {
    #[serde(rename = "mcpServers")]
    mcp_servers: BTreeMap<String, McpServerEntry>,
}

/// Builds the `.mcp.json` body that wires the `kodabi` stdio server into Claude
/// Code, mirroring `.mcp.json.example`: `command` is the resolved binary, `args`
/// is empty, and `env` carries the settings `kodabi-mcp` reads at startup
/// (`crates/kodabi-mcp/src/config.rs`). An entry with no `type`/`url` field is
/// read by Claude Code as a stdio server.
///
/// Regenerated on every terminal and chat open, so a moved vault or a changed
/// aging threshold is picked up without the user knowing this file exists.
pub fn mcp_config_json(paths: &McpPaths) -> serde_json::Result<String> {
    let mut env = BTreeMap::new();
    env.insert("KODABI_INDEX_DB".to_owned(), path_string(&paths.index_db));
    env.insert("KODABI_KB_ROOT".to_owned(), path_string(&paths.kb_root));
    env.insert("KODABI_LEDGER_DB".to_owned(), path_string(&paths.ledger_db));
    env.insert(
        "KODABI_AGING_AFTER_DAYS".to_owned(),
        paths.aging.aging_after_days.to_string(),
    );
    env.insert(
        "KODABI_STALE_AFTER_DAYS".to_owned(),
        paths.aging.stale_after_days.to_string(),
    );

    let mut servers = BTreeMap::new();
    servers.insert(
        MCP_SERVER_KEY.to_owned(),
        McpServerEntry {
            command: path_string(&paths.mcp_binary),
            args: Vec::new(),
            env,
        },
    );

    serde_json::to_string_pretty(&McpConfigFile {
        mcp_servers: servers,
    })
}

#[derive(Serialize)]
struct Permissions {
    allow: Vec<String>,
}

#[derive(Serialize)]
struct SettingsFile {
    permissions: Permissions,
}

/// Builds the Claude Code settings body passed via `--settings`, pre-allowing
/// exactly the read tools ([`READ_TOOL_PERMISSIONS`]) so chat-over-the-KB works
/// with no permission prompt. Writes are not listed, so Claude Code still
/// prompts for them.
pub fn settings_json() -> serde_json::Result<String> {
    serde_json::to_string_pretty(&SettingsFile {
        permissions: Permissions {
            allow: READ_TOOL_PERMISSIONS
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect(),
        },
    })
}

/// The argv (after the program name) for an *interactive* `claude` launch wired
/// to the generated MCP config and settings. Deliberately the inverse of the
/// headless runner in `kodabi-llm`: no `-p`, no `--output-format json` — there
/// is a real terminal, and a write's permission prompt has a TTY to answer.
///
/// `--strict-mcp-config` loads only Kodabi's server, ignoring any user or
/// project `.mcp.json`. (The headless runner uses it too, verified against a
/// live `claude`; if a future CLI drops it, this is the one line to change.)
/// The knowledge-base root, the child env, and the working directory are the
/// shell's job — they are not argv.
pub fn claude_argv(mcp_config: &Path, settings: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--mcp-config"),
        mcp_config.as_os_str().to_os_string(),
        OsString::from("--strict-mcp-config"),
        OsString::from("--settings"),
        settings.as_os_str().to_os_string(),
    ]
}

/// Validates a resize request from the frontend: rejects a zero dimension (a
/// PTY can't be 0 wide/tall, and xterm briefly reports 0 before first layout),
/// and clamps to [`MAX_DIMENSION`]. `None` means "ignore this resize".
pub fn valid_resize(cols: u16, rows: u16) -> Option<(u16, u16)> {
    if cols == 0 || rows == 0 {
        return None;
    }
    Some((cols.min(MAX_DIMENSION), rows.min(MAX_DIMENSION)))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Cap on the replayed scrollback ring: enough to re-hydrate a re-mounted xterm
/// without unbounded growth for a long-lived session.
pub const SCROLLBACK_CAP: usize = 256 * 1024;
/// Flush the coalescer once this much output has piled up, so one big burst
/// (a build, `ls -R`) can't grow a single event without bound.
pub const MAX_EVENT_BYTES: usize = 64 * 1024;
/// Coalesce window: batches the fast small writes of a redrawing TUI into a few
/// events instead of hundreds, bounding the webview's event rate.
pub const COALESCE_WINDOW: Duration = Duration::from_millis(8);
/// One read from the PTY master.
pub const READ_BUF: usize = 8192;

/// The byte pump's tunables. Injected rather than read from the constants above
/// so tests drive [`run_coalescer`] with short timings and tiny caps; the shell
/// passes [`StreamLimits::default`].
#[derive(Debug, Clone, Copy)]
pub struct StreamLimits {
    /// How long to wait for more output before flushing what has piled up.
    pub coalesce: Duration,
    /// Flush once the pending batch has reached this size.
    pub max_event_bytes: usize,
    /// Retain at most this many bytes of scrollback.
    pub scrollback_cap: usize,
}

impl Default for StreamLimits {
    fn default() -> Self {
        Self {
            coalesce: COALESCE_WINDOW,
            max_event_bytes: MAX_EVENT_BYTES,
            scrollback_cap: SCROLLBACK_CAP,
        }
    }
}

/// The shared, observable state of one session's byte stream: what a re-mounted
/// terminal replays, and the two liveness flags the command handlers read.
/// Cloning shares the same underlying state — the shell keeps one handle in its
/// session slot and moves a clone into the coalescer thread.
#[derive(Clone)]
pub struct SessionStream {
    /// Ring of recent raw PTY bytes, capped at [`StreamLimits::scrollback_cap`].
    pub scrollback: Arc<Mutex<VecDeque<u8>>>,
    /// Cleared by [`run_coalescer`] when the child exits, so the next open
    /// respawns rather than reusing a dead session.
    pub alive: Arc<AtomicBool>,
    /// Set by a deliberate reap (restart / app exit) so [`run_coalescer`] stays
    /// silent instead of reporting a spurious exit.
    pub reaped: Arc<AtomicBool>,
}

impl SessionStream {
    /// A live, un-reaped stream with an empty ring.
    pub fn new() -> Self {
        Self {
            scrollback: Arc::new(Mutex::new(VecDeque::new())),
            alive: Arc::new(AtomicBool::new(true)),
            reaped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the child is still running.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Marks a deliberate teardown: the session is no longer live, and its exit
    /// must not be reported.
    pub fn mark_reaped(&self) {
        self.reaped.store(true, Ordering::Relaxed);
        self.alive.store(false, Ordering::Relaxed);
    }

    /// A copy of the retained scrollback. A poisoned ring yields nothing to
    /// replay rather than propagating the panic into a command handler.
    pub fn contents(&self) -> Vec<u8> {
        self.scrollback
            .lock()
            .map(|ring| ring.iter().copied().collect())
            .unwrap_or_default()
    }
}

impl Default for SessionStream {
    fn default() -> Self {
        Self::new()
    }
}

/// Drains `reader` into `tx` in [`READ_BUF`] chunks until the PTY reports EOF
/// (the child exited and the slave was dropped), the read fails, or the receiver
/// is gone. Blocking, so the shell runs it on its own thread.
pub fn pump_reader(mut reader: impl Read, tx: Sender<Vec<u8>>) {
    let mut buf = [0u8; READ_BUF];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
}

/// Batches the reader's chunks into output events and the scrollback ring, then
/// reports the child's exit.
///
/// Each flush appends to the ring (evicting oldest bytes past the cap) and hands
/// the batch to `on_output` — the shell base64-encodes it and emits, since the
/// encoding exists only for the IPC boundary. Once the channel disconnects, the
/// pending batch is flushed, `wait` reaps the child's status, the stream is
/// marked dead, and `on_exit` fires **unless** the session was deliberately
/// reaped: a restart or an app exit must not look like `claude` quitting.
///
/// Blocking, so the shell runs it on its own thread.
pub fn run_coalescer(
    rx: Receiver<Vec<u8>>,
    limits: StreamLimits,
    stream: &SessionStream,
    wait: impl FnOnce() -> Option<i32>,
    mut on_output: impl FnMut(&[u8]),
    on_exit: impl FnOnce(Option<i32>),
) {
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match rx.recv_timeout(limits.coalesce) {
            Ok(chunk) => {
                pending.extend_from_slice(&chunk);
                if pending.len() >= limits.max_event_bytes {
                    flush(stream, limits.scrollback_cap, &mut pending, &mut on_output);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                flush(stream, limits.scrollback_cap, &mut pending, &mut on_output)
            }
            Err(RecvTimeoutError::Disconnected) => {
                flush(stream, limits.scrollback_cap, &mut pending, &mut on_output);
                break;
            }
        }
    }

    let code = wait();
    stream.alive.store(false, Ordering::Relaxed);
    if !stream.reaped.load(Ordering::Relaxed) {
        on_exit(code);
    }
}

/// Appends the pending batch to the ring (oldest bytes first out past `cap`) and
/// hands it to the sink. The ring is updated before the sink runs, so a terminal
/// re-mounting mid-flush replays what it is about to receive. A quiet coalesce
/// window leaves `pending` empty, which must not produce an event.
fn flush(
    stream: &SessionStream,
    cap: usize,
    pending: &mut Vec<u8>,
    on_output: &mut impl FnMut(&[u8]),
) {
    if pending.is_empty() {
        return;
    }
    // A poisoned ring costs the replay, not the live stream: still emit.
    if let Ok(mut ring) = stream.scrollback.lock() {
        ring.extend(pending.iter().copied());
        while ring.len() > cap {
            ring.pop_front();
        }
    }
    on_output(pending);
    pending.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_paths() -> McpPaths {
        McpPaths {
            mcp_binary: PathBuf::from("C:/app/resources/kodabi-mcp.exe"),
            index_db: PathBuf::from("C:/app-data/index.db"),
            kb_root: PathBuf::from("C:/vault"),
            ledger_db: PathBuf::from("C:/app-config/ledger.db"),
            aging: crate::ledger::AgingConfig {
                aging_after_days: 21,
                stale_after_days: 45,
            },
        }
    }

    #[test]
    fn mcp_config_has_the_documented_wiring_shape() {
        let json = mcp_config_json(&sample_paths()).expect("serializes");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");

        let server = &value["mcpServers"]["kodabi"];
        assert_eq!(server["command"], "C:/app/resources/kodabi-mcp.exe");
        assert_eq!(server["args"], serde_json::json!([]));
        assert_eq!(server["env"]["KODABI_INDEX_DB"], "C:/app-data/index.db");
        assert_eq!(server["env"]["KODABI_KB_ROOT"], "C:/vault");
        assert_eq!(server["env"]["KODABI_LEDGER_DB"], "C:/app-config/ledger.db");
        // The user's own thresholds, not the defaults, and stringified because
        // an `env` block's values are strings.
        assert_eq!(server["env"]["KODABI_AGING_AFTER_DAYS"], "21");
        assert_eq!(server["env"]["KODABI_STALE_AFTER_DAYS"], "45");
        // No `type`/`url` field → Claude Code reads it as a stdio server.
        assert!(server.get("type").is_none());
        assert!(server.get("url").is_none());
    }

    #[test]
    fn settings_allow_the_read_tools_and_no_write_tool() {
        let json = settings_json().expect("serializes");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let allow = value["permissions"]["allow"]
            .as_array()
            .expect("allow is an array");

        for read_tool in READ_TOOL_PERMISSIONS {
            assert!(
                allow.iter().any(|entry| entry == read_tool),
                "expected {read_tool} to be pre-approved"
            );
        }
        // The write tools must NOT be pre-approved — they still prompt.
        for write_tool in [
            "mcp__kodabi__file_note_to_project",
            "mcp__kodabi__add_glossary_term",
            "mcp__kodabi__update_action_item",
        ] {
            assert!(
                !allow.iter().any(|entry| entry == write_tool),
                "write tool {write_tool} must not be pre-approved"
            );
        }
    }

    #[test]
    fn claude_argv_is_interactive_and_wires_the_config() {
        let argv = claude_argv(
            Path::new("C:/cfg/kodabi.mcp.json"),
            Path::new("C:/cfg/settings.json"),
        );

        assert!(argv.iter().any(|arg| arg == "--mcp-config"));
        assert!(argv.iter().any(|arg| arg == "C:/cfg/kodabi.mcp.json"));
        assert!(argv.iter().any(|arg| arg == "--settings"));
        assert!(argv.iter().any(|arg| arg == "C:/cfg/settings.json"));
        assert!(argv.iter().any(|arg| arg == "--strict-mcp-config"));

        // Interactive: the headless hermetic flags must be absent.
        assert!(!argv.iter().any(|arg| arg == "-p"));
        assert!(!argv.iter().any(|arg| arg == "--output-format"));
    }

    #[test]
    fn resize_rejects_zero_dimensions() {
        assert_eq!(valid_resize(0, 24), None);
        assert_eq!(valid_resize(80, 0), None);
    }

    #[test]
    fn resize_passes_through_and_clamps() {
        assert_eq!(valid_resize(80, 24), Some((80, 24)));
        assert_eq!(
            valid_resize(5000, 5000),
            Some((MAX_DIMENSION, MAX_DIMENSION))
        );
    }

    /// Roomy limits, so only an explicit disconnect ends a batch.
    fn test_limits() -> StreamLimits {
        StreamLimits {
            coalesce: Duration::from_millis(50),
            max_event_bytes: 1024,
            scrollback_cap: 1024,
        }
    }

    /// Runs the pump synchronously over a pre-loaded channel: with the sender
    /// dropped up front, `recv_timeout` drains the queue without waiting, then
    /// sees the disconnect. Returns the flushed batches and the exit report,
    /// where the outer `None` means `on_exit` never fired (a reaped session).
    #[allow(clippy::type_complexity)]
    fn drive(
        limits: StreamLimits,
        stream: &SessionStream,
        chunks: &[&[u8]],
        wait: impl FnOnce() -> Option<i32>,
    ) -> (Vec<Vec<u8>>, Option<Option<i32>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        for chunk in chunks {
            tx.send(chunk.to_vec()).expect("receiver is live");
        }
        drop(tx);

        let mut events = Vec::new();
        let mut exit = None;
        run_coalescer(
            rx,
            limits,
            stream,
            wait,
            |batch| events.push(batch.to_vec()),
            |code| exit = Some(code),
        );
        (events, exit)
    }

    #[test]
    fn a_preloaded_burst_flushes_once_as_one_event() {
        let stream = SessionStream::new();
        let (events, exit) = drive(test_limits(), &stream, &[b"ab", b"cd", b"ef"], || Some(0));

        assert_eq!(events, vec![b"abcdef".to_vec()]);
        assert_eq!(stream.contents(), b"abcdef".to_vec());
        assert_eq!(exit, Some(Some(0)));
        assert!(!stream.is_alive());
    }

    #[test]
    fn reaching_max_event_bytes_splits_the_burst() {
        let limits = StreamLimits {
            max_event_bytes: 4,
            ..test_limits()
        };
        let stream = SessionStream::new();
        let (events, _) = drive(limits, &stream, &[b"abcd", b"ef"], || Some(0));

        assert_eq!(events, vec![b"abcd".to_vec(), b"ef".to_vec()]);
        assert_eq!(stream.contents(), b"abcdef".to_vec());
    }

    #[test]
    fn one_oversized_chunk_still_flushes_whole() {
        // The size check runs after the chunk is appended, so a single read
        // larger than the cap goes out intact rather than being split.
        let limits = StreamLimits {
            max_event_bytes: 4,
            ..test_limits()
        };
        let stream = SessionStream::new();
        let (events, _) = drive(limits, &stream, &[b"abcdefgh"], || Some(0));

        assert_eq!(events, vec![b"abcdefgh".to_vec()]);
    }

    #[test]
    fn quiet_gaps_split_output_into_separate_events() {
        let limits = StreamLimits {
            coalesce: Duration::from_millis(25),
            ..test_limits()
        };
        let stream = SessionStream::new();
        let stream_for_thread = stream.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let (events_tx, events_rx) = std::sync::mpsc::channel();

        let handle = std::thread::spawn(move || {
            run_coalescer(
                rx,
                limits,
                &stream_for_thread,
                || Some(0),
                |batch| {
                    let _ = events_tx.send(batch.to_vec());
                },
                |_| {},
            );
        });

        // The gap is a rendezvous, not a sleep: the second chunk goes in only
        // once the first has actually been flushed, so the split can't hinge on
        // the pump thread being scheduled inside a fixed window. The receive
        // itself is the assertion — nothing but the quiet-window flush can
        // produce an event here, since neither `max_event_bytes` nor the
        // disconnect has been reached. Its timeout is a failure bound, not a
        // synchronisation delay.
        tx.send(b"a".to_vec()).expect("pump is live");
        let first = events_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the quiet window flushes the first chunk on its own");
        tx.send(b"b".to_vec()).expect("pump is live");
        drop(tx);

        handle.join().expect("pump thread finishes");
        // The sink's sender died with the pump thread, so this drains and stops.
        let rest: Vec<Vec<u8>> = events_rx.iter().collect();

        assert_eq!(first, b"a".to_vec());
        assert_eq!(rest, vec![b"b".to_vec()]);
    }

    #[test]
    fn quiet_timeouts_emit_no_empty_events() {
        let limits = StreamLimits {
            coalesce: Duration::from_millis(25),
            ..test_limits()
        };
        let stream = SessionStream::new();
        let stream_for_thread = stream.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();

        let handle = std::thread::spawn(move || {
            let mut events = 0usize;
            let mut exits = 0usize;
            run_coalescer(
                rx,
                limits,
                &stream_for_thread,
                || Some(0),
                |_| events += 1,
                |_| exits += 1,
            );
            (events, exits)
        });

        // Several coalesce windows pass with nothing to flush.
        std::thread::sleep(Duration::from_millis(100));
        drop(tx);

        let (events, exits) = handle.join().expect("pump thread finishes");
        assert_eq!(events, 0, "a quiet window must not emit an empty event");
        assert_eq!(exits, 1);
    }

    #[test]
    fn the_ring_evicts_oldest_bytes_beyond_the_cap() {
        let limits = StreamLimits {
            scrollback_cap: 8,
            ..test_limits()
        };

        let stream = SessionStream::new();
        let (events, _) = drive(limits, &stream, &[b"abcdefghijkl"], || Some(0));
        // The event carries everything; only the replay ring is capped.
        assert_eq!(events, vec![b"abcdefghijkl".to_vec()]);
        assert_eq!(stream.contents(), b"efghijkl".to_vec());

        // Exactly at the cap nothing is evicted (the boundary is strictly >).
        let exact = SessionStream::new();
        drive(limits, &exact, &[b"abcdefgh"], || Some(0));
        assert_eq!(exact.contents(), b"abcdefgh".to_vec());
    }

    #[test]
    fn a_natural_exit_reports_the_code() {
        let stream = SessionStream::new();
        let (_, exit) = drive(test_limits(), &stream, &[], || Some(3));

        assert_eq!(exit, Some(Some(3)));
        assert!(!stream.is_alive());
    }

    #[test]
    fn a_wait_error_reports_no_code() {
        let stream = SessionStream::new();
        let (_, exit) = drive(test_limits(), &stream, &[], || None);

        // Reported with an unknown code — distinct from not reported at all.
        assert_eq!(exit, Some(None));
    }

    #[test]
    fn a_reaped_session_suppresses_the_exit_report() {
        let stream = SessionStream::new();
        stream.mark_reaped();
        let (events, exit) = drive(test_limits(), &stream, &[b"bye"], || Some(0));

        assert_eq!(exit, None, "a deliberate reap must stay silent");
        // Output buffered before the reap still reaches the ring and the sink.
        assert_eq!(events, vec![b"bye".to_vec()]);
        assert_eq!(stream.contents(), b"bye".to_vec());
        assert!(!stream.is_alive());
    }

    #[test]
    fn session_stream_lifecycle_flags() {
        let stream = SessionStream::new();
        assert!(stream.is_alive());
        assert!(!stream.reaped.load(Ordering::Relaxed));

        stream.mark_reaped();
        assert!(!stream.is_alive());
        assert!(stream.reaped.load(Ordering::Relaxed));
    }

    #[test]
    fn pump_reader_chunks_and_stops_at_eof() {
        let source = vec![7u8; READ_BUF * 2 + 512];
        let (tx, rx) = std::sync::mpsc::channel();
        pump_reader(std::io::Cursor::new(source.clone()), tx);

        let chunks: Vec<Vec<u8>> = rx.iter().collect();
        assert!(chunks.len() >= 3, "a read is capped at READ_BUF");
        assert!(chunks.iter().all(|chunk| chunk.len() <= READ_BUF));
        assert_eq!(chunks.concat(), source);
    }

    #[test]
    fn pump_reader_stops_on_read_error() {
        struct FailingReader;
        impl Read for FailingReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("pty closed"))
            }
        }

        let (tx, rx) = std::sync::mpsc::channel();
        pump_reader(FailingReader, tx);

        assert_eq!(rx.iter().count(), 0);
    }

    #[test]
    fn pump_reader_stops_when_the_receiver_is_gone() {
        use std::sync::atomic::AtomicUsize;

        /// A source that keeps yielding, so returning at all is evidence of the
        /// send check rather than of the reader running dry. Bounded anyway, so
        /// a regression fails the count below instead of hanging the suite.
        struct CountingReader(Arc<AtomicUsize>);
        impl Read for CountingReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.fetch_add(1, Ordering::Relaxed) >= 8 {
                    return Ok(0);
                }
                buf.fill(1);
                Ok(buf.len())
            }
        }

        let reads = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);

        // Returns rather than spinning on a dead channel.
        pump_reader(CountingReader(Arc::clone(&reads)), tx);

        assert_eq!(
            reads.load(Ordering::Relaxed),
            1,
            "the first failed send must end the pump, not the end of the source"
        );
    }
}
