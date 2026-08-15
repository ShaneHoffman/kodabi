//! The chat session's process side: the second sanctioned `claude` spawn in
//! the tree, beside the one-shot distill runner in the crate root.
//!
//! Where [`crate::ClaudeRunner`] is blocking `String → String`
//! (`kodabi_core::llm::HeadlessClaude`), a chat session is a *long-lived*
//! bidirectional stream-json process: user messages and permission responses
//! go in on stdin, parsed [`ChatStreamItem`]s come out on a channel, and the
//! process carries the whole multi-turn conversation. That shape cannot be
//! expressed through `HeadlessClaude`, so this is a seam beside it, not an
//! implementation of it.
//!
//! House style throughout: `std::thread` + `mpsc`, no async runtime. Three
//! threads per child, each owning exactly one pipe:
//!
//! - **stdin writer** — channel-fed, so a full OS pipe buffer can never block
//!   a caller (the same deadlock reasoning as `wait_with_timeout`'s dedicated
//!   stdin thread in the crate root).
//! - **stdout reader** — `BufReader::lines` → [`parse_stream_line`] → events
//!   channel. After EOF it collects the exit code from the waiter and sends
//!   [`ChatChildEvent::Exited`] as the guaranteed-last event.
//! - **waiter** — `child.wait()`, feeding the reader the exit code and
//!   flipping the shared `alive` flag. (stderr is inherited into the void via
//!   `Stdio::null`; spawn-time failures surface through the spawn call
//!   itself, and runtime diagnostics flow through the stream as `result`
//!   lines.)

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use kodabi_core::chat::{chat_argv, parse_stream_line, ChatStreamItem, CHAT_DEFAULT_MODEL};
use kodabi_core::terminal::SKIP_HISTORY_ENV;

/// Configuration for a chat session's `claude` process.
#[derive(Debug, Clone)]
pub struct ChatProcessConfig {
    /// Model alias or full ID passed to `--model`.
    pub model: String,
    /// Explicit path to the `claude` binary. `None` resolves the bare name
    /// `claude` via `PATH` (see the crate docs on Windows `.cmd` shims).
    pub binary: Option<PathBuf>,
}

impl Default for ChatProcessConfig {
    fn default() -> Self {
        Self {
            model: CHAT_DEFAULT_MODEL.to_owned(),
            binary: None,
        }
    }
}

impl ChatProcessConfig {
    /// The chat session's config: [`ChatProcessConfig::default`] with the
    /// `KODABI_CHAT_MODEL` and `KODABI_CLAUDE_BINARY` overrides applied, if
    /// set (the latter mirroring the embedded terminal's binary override).
    pub fn from_env() -> Self {
        Self::from_overrides(
            std::env::var("KODABI_CHAT_MODEL").ok(),
            std::env::var("KODABI_CLAUDE_BINARY").ok(),
        )
    }

    fn from_overrides(model: Option<String>, binary: Option<String>) -> Self {
        let mut config = Self::default();
        if let Some(model) = model {
            if !model.trim().is_empty() {
                config.model = model;
            }
        }
        if let Some(binary) = binary {
            if !binary.trim().is_empty() {
                config.binary = Some(PathBuf::from(binary));
            }
        }
        config
    }
}

/// One event out of a chat child.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatChildEvent {
    /// A parsed stdout item.
    Item(ChatStreamItem),
    /// The process exited. Always the last event on the channel.
    Exited { code: Option<i32> },
}

/// The chat process failed to spawn.
#[derive(Debug, Clone, thiserror::Error)]
#[error("failed to spawn headless Claude Code for chat: {0}")]
pub struct ChatSpawnError(pub String);

/// The chat process is no longer accepting input (its stdin writer is gone,
/// which means the process exited or is exiting).
#[derive(Debug, Clone, thiserror::Error)]
#[error("chat session is not running")]
pub struct ChatWriteError;

/// The write/kill side of a spawned chat `claude`. The event side is the
/// `mpsc::Receiver` returned alongside it by [`spawn_chat`] — the two halves
/// separate so the caller can move the receiver into a pump thread while
/// command handlers keep the writer.
pub struct ChatChild {
    pid: u32,
    stdin_tx: mpsc::Sender<String>,
    alive: Arc<AtomicBool>,
}

impl ChatChild {
    /// The child's process id (the root of the `claude → kodabi-mcp` tree).
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Queues one stream-json line for the child's stdin (the writer thread
    /// appends the newline).
    pub fn write_line(&self, line: String) -> Result<(), ChatWriteError> {
        self.stdin_tx.send(line).map_err(|_| ChatWriteError)
    }

    /// Whether the process is still running (flipped by the waiter thread the
    /// moment `wait` returns).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Kills the process and its whole descendant tree (`claude → kodabi-mcp`),
    /// best-effort. The waiter thread then observes the exit and the reader
    /// emits [`ChatChildEvent::Exited`] as usual.
    pub fn kill(&self) {
        crate::kill_process_tree(self.pid);
    }
}

/// Spawns the chat `claude` wired to the kodabi MCP server, with stdout
/// parsing already running. `mcp_config` is the generated `.mcp.json` path,
/// `session_id` pins Claude Code's session id (the app's chat id), and `cwd`
/// is the knowledge-base root.
///
/// Sets [`SKIP_HISTORY_ENV`] like every Kodabi `claude` spawn (FOUNDING_DOC
/// §3.7) — which is exactly why the caller persists its own transcript.
pub fn spawn_chat(
    config: &ChatProcessConfig,
    mcp_config: &Path,
    session_id: &str,
    cwd: &Path,
) -> Result<(ChatChild, mpsc::Receiver<ChatChildEvent>), ChatSpawnError> {
    let program = config
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("claude"));

    let mut command = crate::hidden_command(program);
    command
        .args(chat_argv(mcp_config, &config.model, session_id))
        .env(SKIP_HISTORY_ENV, "1")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    // A missing `claude` carries the prerequisite message rather than an OS
    // error: this newtype is a string, so the frontend recognises the case by
    // the marker phrase inside it (see `CLAUDE_MISSING_MESSAGE`), and the
    // Display prefix above keeps it a substring.
    let mut child = command.spawn().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            ChatSpawnError(kodabi_core::llm::CLAUDE_MISSING_MESSAGE.to_owned())
        } else {
            ChatSpawnError(err.to_string())
        }
    })?;
    let pid = child.id();

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ChatSpawnError("child has no stdin".to_owned()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ChatSpawnError("child has no stdout".to_owned()))?;

    // stdin writer: owns the pipe; a caller can never block on a full buffer.
    let (stdin_tx, stdin_rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        use std::io::Write;
        let mut stdin = stdin;
        while let Ok(line) = stdin_rx.recv() {
            if stdin.write_all(line.as_bytes()).is_err()
                || stdin.write_all(b"\n").is_err()
                || stdin.flush().is_err()
            {
                // Process gone; drain and let senders see the drop.
                break;
            }
        }
        // `stdin` drops here, closing the pipe.
    });

    // Waiter: reaps the child and reports the exit code to the reader, so
    // `Exited` can be the guaranteed-last event on the channel.
    let alive = Arc::new(AtomicBool::new(true));
    let (code_tx, code_rx) = mpsc::channel::<Option<i32>>();
    let waiter_alive = Arc::clone(&alive);
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|status| status.code());
        waiter_alive.store(false, Ordering::SeqCst);
        let _ = code_tx.send(code);
    });

    // stdout reader: parse each line, forward the items, then the exit.
    let (events_tx, events_rx) = mpsc::channel::<ChatChildEvent>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            let Ok(line) = line else { break };
            for item in parse_stream_line(&line) {
                if events_tx.send(ChatChildEvent::Item(item)).is_err() {
                    return; // receiver gone: session torn down
                }
            }
        }
        let code = code_rx.recv().ok().flatten();
        let _ = events_tx.send(ChatChildEvent::Exited { code });
    });

    Ok((
        ChatChild {
            pid,
            stdin_tx,
            alive,
        },
        events_rx,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_the_chat_model() {
        let config = ChatProcessConfig::default();
        assert_eq!(config.model, CHAT_DEFAULT_MODEL);
        assert!(config.binary.is_none());
    }

    #[test]
    fn overrides_apply_and_blanks_are_ignored() {
        let config = ChatProcessConfig::from_overrides(
            Some("claude-opus-4-8".to_owned()),
            Some("C:/tools/claude.cmd".to_owned()),
        );
        assert_eq!(config.model, "claude-opus-4-8");
        assert_eq!(config.binary, Some(PathBuf::from("C:/tools/claude.cmd")));

        let config = ChatProcessConfig::from_overrides(Some("  ".to_owned()), Some(String::new()));
        assert_eq!(config.model, CHAT_DEFAULT_MODEL);
        assert!(config.binary.is_none());
    }

    #[test]
    fn a_missing_binary_spawns_the_prerequisite_message() {
        // The chat half of the missing-CLI contract: ChatView reads this
        // string, so it must carry the canonical message rather than the OS
        // error. See `kodabi_core::llm::CLAUDE_MISSING_MESSAGE`.
        let dir = tempfile::tempdir().expect("tempdir");
        let config = ChatProcessConfig {
            binary: Some(dir.path().join("no-such-claude")),
            ..ChatProcessConfig::default()
        };

        let err = spawn_chat(&config, &dir.path().join(".mcp.json"), "c_test", dir.path())
            .err()
            .expect("spawn should fail");

        assert_eq!(err.0, kodabi_core::llm::CLAUDE_MISSING_MESSAGE);
        assert!(
            err.to_string()
                .contains(kodabi_core::llm::CLAUDE_MISSING_MESSAGE),
            "Display must keep the message a substring: {err}"
        );
    }
}
