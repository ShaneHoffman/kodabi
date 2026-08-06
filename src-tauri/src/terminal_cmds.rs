//! The embedded Claude Code terminal's PTY orchestration (Phase 3, FOUNDING_DOC §4).
//!
//! The pure argv / `.mcp.json` / settings / resize logic lives in
//! `kodabi_core::terminal`; this module owns only the side-effecting parts that
//! are inherently Tauri-bound — resolving machine paths through `app.path()`,
//! spawning the PTY, streaming its output as events, and reaping the child tree
//! on true app exit. It follows `capture_control.rs` (Tauri-coupled shell code),
//! not `kodabi-llm` (a crate a pure consumer needs): nothing pure consumes the
//! PTY, so there is no crate boundary to earn.
//!
//! One live session at a time, held in [`TerminalState`]. It survives hide-to-
//! tray and view-switches (the webview's xterm is disposed and re-hydrated from
//! the scrollback ring on `terminal_open`); it is reaped only on true app exit
//! (the `RunEvent` hook in `lib.rs`) or an explicit restart. The window's
//! `CloseRequested` only hides to tray, so it must NOT reap here.
//!
//! Concurrency is the house `std::thread` + `mpsc` style (per `kodabi-llm`), not
//! async. Two threads per session: a blocking reader draining the PTY into a
//! channel, and a coalescer that batches, appends to the scrollback ring, and
//! emits `terminal:output`. The slot is mutated only by command handlers under
//! the lock; the coalescer only flips atomics, so there is no slot race.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use kodabi_core::terminal;

/// Live PTY output, base64-encoded raw bytes (a per-chunk UTF-8 decode would
/// corrupt multibyte sequences split across reads). Mirrors `OutputPayload`.
pub const TERMINAL_OUTPUT_EVENT: &str = "terminal:output";
/// The hosted `claude` process exited (naturally — a restart/app-exit reap is
/// silent). Mirrors `ExitPayload`.
pub const TERMINAL_EXIT_EVENT: &str = "terminal:exit";

/// Cap on the replayed scrollback ring: enough to re-hydrate a re-mounted xterm
/// without unbounded growth for a long-lived session.
const SCROLLBACK_CAP: usize = 256 * 1024;
/// Flush the coalescer once this much output has piled up, so one big burst
/// (a build, `ls -R`) can't grow a single event without bound.
const MAX_EVENT_BYTES: usize = 64 * 1024;
const READ_BUF: usize = 8192;
/// Coalesce window: batches the fast small writes of a redrawing TUI into a few
/// events instead of hundreds, bounding the webview's event rate.
const COALESCE_MS: u64 = 8;
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const POISONED: &str = "terminal state lock poisoned";

/// The one live terminal session, or none. Managed at builder level like
/// `CaptureState`. The `Mutex` makes the whole thing `Sync`; the session itself
/// only needs to be `Send`.
#[derive(Default)]
pub struct TerminalState(pub Mutex<Option<TerminalSession>>);

/// A running PTY hosting `claude`. The reader/coalescer threads and the child
/// handle live outside this struct (in the coalescer thread); what stays here is
/// what the command handlers need: the master (resize + keep-alive), the writer
/// (stdin), the pid (reap), the scrollback ring (reattach), and the liveness
/// atomics.
pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pid: Option<u32>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    /// Cleared by the coalescer when the child exits; the next `terminal_open`
    /// sees `false` and respawns rather than reusing a dead session.
    alive: Arc<AtomicBool>,
    /// Set by a deliberate reap (restart / app exit) so the coalescer stays
    /// silent instead of emitting a spurious `terminal:exit`.
    reaped: Arc<AtomicBool>,
    cols: u16,
    rows: u16,
}

impl TerminalSession {
    fn snapshot(&self) -> TerminalSnapshot {
        let scrollback = self
            .scrollback
            .lock()
            .map(|ring| BASE64.encode(ring.iter().copied().collect::<Vec<u8>>()))
            .unwrap_or_default();
        TerminalSnapshot {
            running: self.alive.load(Ordering::Relaxed),
            scrollback,
            cols: self.cols,
            rows: self.rows,
        }
    }

    /// Deliberate teardown: mark it reaped (so the coalescer stays quiet) and
    /// kill the whole `cmd.exe → claude → kodabi-mcp` tree.
    fn reap(&self) {
        self.reaped.store(true, Ordering::Relaxed);
        self.alive.store(false, Ordering::Relaxed);
        if let Some(pid) = self.pid {
            kodabi_llm::kill_process_tree(pid);
        }
    }
}

impl Drop for TerminalSession {
    /// Safety net for a session dropped without an explicit reap (a panic, an
    /// abnormal teardown). Idempotent: `taskkill` on an already-dead pid is a
    /// harmless no-op, and a normal path has already reaped.
    fn drop(&mut self) {
        if let Some(pid) = self.pid {
            kodabi_llm::kill_process_tree(pid);
        }
    }
}

/// Mirrors `TerminalSnapshot` in `src/terminal.ts`. Seeds a freshly mounted
/// xterm: `scrollback` is base64 raw PTY bytes to replay, `cols`/`rows` the
/// current grid.
#[derive(Serialize)]
pub struct TerminalSnapshot {
    running: bool,
    scrollback: String,
    cols: u16,
    rows: u16,
}

#[derive(Clone, Serialize)]
struct OutputPayload {
    data: String,
}

#[derive(Clone, Serialize)]
struct ExitPayload {
    code: Option<i32>,
}

/// Ensures a live session and returns a snapshot to hydrate the terminal.
/// Idempotent: reuses the running session (replaying its scrollback) so a view
/// switch or hide-to-tray does not restart `claude`; only a missing or dead
/// session spawns a new one.
#[tauri::command]
pub fn terminal_open(
    app: AppHandle,
    state: State<'_, TerminalState>,
) -> Result<TerminalSnapshot, String> {
    let mut guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    if let Some(session) = guard.as_ref() {
        if session.alive.load(Ordering::Relaxed) {
            return Ok(session.snapshot());
        }
    }
    // Drop any dead session (Drop reaps its pid — a no-op if already gone).
    guard.take();
    let session = spawn_session(&app)?;
    let snapshot = session.snapshot();
    *guard = Some(session);
    Ok(snapshot)
}

/// Writes keyboard input (xterm's `onData` string) to the PTY. A no-op if no
/// session is live, so a stray keystroke after exit can't error.
#[tauri::command]
pub fn terminal_write(state: State<'_, TerminalState>, data: String) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    if let Some(session) = guard.as_mut() {
        if session.alive.load(Ordering::Relaxed) {
            session
                .writer
                .write_all(data.as_bytes())
                .map_err(|e| e.to_string())?;
            session.writer.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Resizes the PTY grid to match the xterm viewport. Rejects/clamps via
/// [`terminal::valid_resize`]; a no-op if no session is live.
#[tauri::command]
pub fn terminal_resize(
    state: State<'_, TerminalState>,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let Some((cols, rows)) = terminal::valid_resize(cols, rows) else {
        return Ok(());
    };
    let mut guard = state.0.lock().map_err(|_| POISONED.to_string())?;
    if let Some(session) = guard.as_mut() {
        session
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        session.cols = cols;
        session.rows = rows;
    }
    Ok(())
}

/// Reaps the current session (if any) and spawns a fresh one — the "Restart"
/// action after `claude` exits, or a deliberate reset.
#[tauri::command]
pub fn terminal_restart(
    app: AppHandle,
    state: State<'_, TerminalState>,
) -> Result<TerminalSnapshot, String> {
    {
        let mut guard = state.0.lock().map_err(|_| POISONED.to_string())?;
        if let Some(session) = guard.take() {
            session.reap();
        }
    }
    let session = spawn_session(&app)?;
    let snapshot = session.snapshot();
    *state.0.lock().map_err(|_| POISONED.to_string())? = Some(session);
    Ok(snapshot)
}

/// Reaps the live session on true app exit. Called from the `RunEvent` hook in
/// `lib.rs` — NOT from `WindowEvent::CloseRequested`, which only hides to tray.
pub fn reap(app: &AppHandle) {
    if let Some(state) = app.try_state::<TerminalState>() {
        if let Ok(mut guard) = state.0.lock() {
            if let Some(session) = guard.take() {
                session.reap();
            }
        }
    }
}

/// Spawns the PTY, writes the generated MCP config + settings, launches `claude`
/// wired to them, and starts the reader + coalescer threads.
fn spawn_session(app: &AppHandle) -> Result<TerminalSession, String> {
    let (mcp_path, settings_path) = write_config_files(app)?;
    let kb_root = crate::transcribe::knowledge_base_dir(app)?;

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let command = build_command(&mcp_path, &settings_path, &kb_root);
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| e.to_string())?;
    let pid = child.process_id();
    // The slave is only needed to spawn against; dropping it now means the PTY
    // reports EOF once the child exits.
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    let master = pair.master;

    let scrollback = Arc::new(Mutex::new(VecDeque::<u8>::new()));
    let alive = Arc::new(AtomicBool::new(true));
    let reaped = Arc::new(AtomicBool::new(false));

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut reader = reader;
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
    });

    let coalesce_app = app.clone();
    let coalesce_scrollback = Arc::clone(&scrollback);
    let coalesce_alive = Arc::clone(&alive);
    let coalesce_reaped = Arc::clone(&reaped);
    std::thread::spawn(move || {
        run_coalescer(
            coalesce_app,
            rx,
            coalesce_scrollback,
            coalesce_alive,
            coalesce_reaped,
            child,
        );
    });

    Ok(TerminalSession {
        master,
        writer,
        pid,
        scrollback,
        alive,
        reaped,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    })
}

/// Drains the reader channel, batching output into `terminal:output` events and
/// the scrollback ring, then waits for the child and (unless reaped) emits
/// `terminal:exit`.
fn run_coalescer(
    app: AppHandle,
    rx: mpsc::Receiver<Vec<u8>>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    alive: Arc<AtomicBool>,
    reaped: Arc<AtomicBool>,
    mut child: Box<dyn Child + Send + Sync>,
) {
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(COALESCE_MS)) {
            Ok(chunk) => {
                pending.extend_from_slice(&chunk);
                if pending.len() >= MAX_EVENT_BYTES {
                    flush(&app, &scrollback, &mut pending);
                }
            }
            Err(RecvTimeoutError::Timeout) => flush(&app, &scrollback, &mut pending),
            Err(RecvTimeoutError::Disconnected) => {
                flush(&app, &scrollback, &mut pending);
                break;
            }
        }
    }

    let code = child.wait().ok().map(|status| status.exit_code() as i32);
    alive.store(false, Ordering::Relaxed);
    if !reaped.load(Ordering::Relaxed) {
        let _ = app.emit(TERMINAL_EXIT_EVENT, ExitPayload { code });
    }
}

fn flush(app: &AppHandle, scrollback: &Arc<Mutex<VecDeque<u8>>>, pending: &mut Vec<u8>) {
    if pending.is_empty() {
        return;
    }
    if let Ok(mut ring) = scrollback.lock() {
        ring.extend(pending.iter().copied());
        while ring.len() > SCROLLBACK_CAP {
            ring.pop_front();
        }
    }
    let data = BASE64.encode(&pending);
    let _ = app.emit(TERMINAL_OUTPUT_EVENT, OutputPayload { data });
    pending.clear();
}

/// Builds the `claude` launch. On Windows this goes through `cmd.exe /C` because
/// `portable_pty::CommandBuilder` calls `CreateProcess` directly and does NOT
/// shell an npm `claude.cmd` the way `std::process::Command` does (the
/// CVE-2024-24576 fix `kodabi-llm` relies on). cwd is the KB root so `claude`'s
/// file tools reach the vault directly; `CLAUDE_CODE_SKIP_PROMPT_HISTORY`
/// disables its own transcript logging (FOUNDING_DOC §3.7).
fn build_command(mcp_path: &Path, settings_path: &Path, cwd: &Path) -> CommandBuilder {
    let argv = terminal::claude_argv(mcp_path, settings_path);
    let mut command = base_command(&argv);
    command.cwd(cwd);
    command.env(terminal::SKIP_HISTORY_ENV, "1");
    command
}

#[cfg(windows)]
fn base_command(argv: &[OsString]) -> CommandBuilder {
    let shell = std::env::var_os("ComSpec").unwrap_or_else(|| OsString::from("cmd.exe"));
    let mut command = CommandBuilder::new(shell);
    command.arg("/C");
    command.arg(claude_program());
    command.args(argv.iter());
    command
}

#[cfg(not(windows))]
fn base_command(argv: &[OsString]) -> CommandBuilder {
    let mut command = CommandBuilder::new(claude_program());
    command.args(argv.iter());
    command
}

/// The `claude` program to launch. Resolved on PATH by default; overridable via
/// `KODABI_CLAUDE_BINARY` for a non-standard install.
fn claude_program() -> OsString {
    std::env::var_os("KODABI_CLAUDE_BINARY").unwrap_or_else(|| OsString::from("claude"))
}

/// The one directory holding every generated Claude Code wiring file. The
/// underscore prefix is load-bearing: on Windows, `app_config_dir()` and
/// `app_data_dir()` are the SAME folder, and that folder is the KB root — so a
/// plainly named config dir here becomes a phantom project in `list_projects`
/// (which is exactly what the earlier `mcp/` and `terminal/` dirs did).
/// Vault enumeration skips `_`/`.`-prefixed dirs as infra, the same shield
/// `EBWebView` needs a reserved name for.
const WIRING_DIR: &str = "_claude";

/// Generates the machine-local `.mcp.json` under [`WIRING_DIR`] in the app's
/// config dir (it holds absolute machine paths, so it must never sync with the
/// KB's content) and returns its path. Regenerated on each open so a moved
/// install is self-healing. Shared with the chat session (`chat_cmds`), which
/// wires the same server into its headless spawn.
///
/// All three paths come from the app's own resolvers rather than from
/// `app_config_dir()`/`app_data_dir()` inline, so the sidecar is handed the same
/// vault, the same index and the same config dir this process opened — under the
/// dev sandbox that means the sidecar lands in the sandbox too (`crate::sandbox`).
/// `KODABI_KB_ROOT` and `KODABI_INDEX_DB` move together or not at all (see
/// `index_state::index_db_path`); resolving one here and hard-coding the other
/// would split the sidecar's reads from its writes.
pub(crate) fn write_mcp_config(app: &AppHandle) -> Result<PathBuf, String> {
    let mcp_binary = resolve_mcp_binary(app)?;
    let config_dir = crate::sandbox::config_dir(app)?;
    let index_db = crate::index_state::index_db_path(app)?;
    let kb_root = crate::transcribe::knowledge_base_dir(app)?;
    remove_legacy_wiring(&config_dir);

    let paths = terminal::McpPaths {
        mcp_binary,
        index_db,
        kb_root,
    };
    let mcp_json = terminal::mcp_config_json(&paths).map_err(|e| e.to_string())?;

    let wiring_dir = config_dir.join(WIRING_DIR);
    fs::create_dir_all(&wiring_dir).map_err(|e| e.to_string())?;
    let mcp_path = wiring_dir.join("kodabi.mcp.json");
    fs::write(&mcp_path, mcp_json).map_err(|e| e.to_string())?;
    Ok(mcp_path)
}

/// The terminal's config pair: the shared `.mcp.json` plus the Claude Code
/// settings file pre-approving the read tools (the chat spawn passes its
/// allow-list as argv instead, so the settings file stays terminal-only).
fn write_config_files(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let mcp_path = write_mcp_config(app)?;
    let config_dir = crate::sandbox::config_dir(app)?;

    let settings_json = terminal::settings_json().map_err(|e| e.to_string())?;
    let settings_path = config_dir.join(WIRING_DIR).join("terminal-settings.json");
    fs::write(&settings_path, settings_json).map_err(|e| e.to_string())?;
    Ok((mcp_path, settings_path))
}

/// Removes the wiring files' pre-`_claude` homes (`mcp/kodabi.mcp.json`,
/// `terminal/settings.json`), which sat in the shared config/data folder as
/// bare dirs and therefore listed as phantom "mcp" and "terminal" projects on
/// Windows (see [`WIRING_DIR`]). Best-effort and surgical: only the known
/// files are removed, and `remove_dir` refuses a dir holding anything else —
/// a user's real `mcp/` or `terminal/` project keeps its notes.
fn remove_legacy_wiring(config_dir: &Path) {
    for (dir, file) in [("mcp", "kodabi.mcp.json"), ("terminal", "settings.json")] {
        let legacy_dir = config_dir.join(dir);
        let _ = fs::remove_file(legacy_dir.join(file));
        let _ = fs::remove_dir(&legacy_dir);
    }
}

/// Resolves the `kodabi-mcp` binary: an explicit `KODABI_MCP_BINARY` override,
/// then a sibling of this executable (dev and release-from-source: `pnpm
/// mcp:build*` builds it into the workspace `target/<profile>/`, beside the app
/// exe), then the bundled resource dir.
///
/// An installed copy resolves through the last two branches: the installer
/// carries `kodabi-mcp.exe` at the resource-dir root via `bundle.resources` in
/// `src-tauri/tauri.bundle.conf.json`, and on Windows that root *is* the install
/// directory, so the sibling branch above already finds it — the resource branch
/// is the general case for a layout where the two differ. That overlay is
/// applied only by the `pnpm tauri:build` script (`--config`), deliberately —
/// `tauri-build` copies and *validates* every `bundle.resources` path at every
/// `src-tauri` compile, and the bare cargo gates (`cargo clippy/test
/// --workspace`, CI's Rust jobs, `pnpm tauri dev`) never build the release
/// `kodabi-mcp`, so listing it in the base `tauri.conf.json` would fail them all
/// on a missing file. A bare `tauri build` skips the overlay and ships no
/// sidecar: build installers with `pnpm tauri:build`.
fn resolve_mcp_binary(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os("KODABI_MCP_BINARY") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(sibling) = exe.parent().map(|dir| dir.join(MCP_BINARY_NAME)) {
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }
    if let Ok(resources) = app.path().resource_dir() {
        let bundled = resources.join(MCP_BINARY_NAME);
        if bundled.is_file() {
            return Ok(bundled);
        }
    }
    Err(format!(
        "could not locate {MCP_BINARY_NAME}; build it (cargo build -p kodabi-mcp) or set KODABI_MCP_BINARY"
    ))
}

#[cfg(windows)]
const MCP_BINARY_NAME: &str = "kodabi-mcp.exe";
#[cfg(not(windows))]
const MCP_BINARY_NAME: &str = "kodabi-mcp";
