//! The embedded Claude Code terminal's PTY orchestration (Phase 3, FOUNDING_DOC §4).
//!
//! The pure argv / `.mcp.json` / settings / resize logic lives in
//! `kodabi_core::terminal`, and so does the byte pump that batches PTY output
//! into the scrollback ring — parameterized on its channel and sinks, the way
//! `kodabi_core::watch`'s debounce loop is, so it is unit-tested without a PTY.
//! This module owns what stays inherently Tauri-bound: resolving machine paths
//! through `app.path()`, spawning the PTY, emitting the pump's batches as
//! events, and reaping the child tree on true app exit.
//!
//! One live session at a time, held in [`TerminalState`]. It survives hide-to-
//! tray and view-switches (the webview's xterm is disposed and re-hydrated from
//! the scrollback ring on `terminal_open`); it is reaped only on true app exit
//! (the `RunEvent` hook in `lib.rs`) or an explicit restart. The window's
//! `CloseRequested` only hides to tray, so it must NOT reap here.
//!
//! Concurrency is the house `std::thread` + `mpsc` style (per `kodabi-llm`), not
//! async. Two threads per session, both running a core loop: a blocking reader
//! draining the PTY into a channel ([`terminal::pump_reader`]), and a coalescer
//! ([`terminal::run_coalescer`]) whose sinks emit `terminal:output` and
//! `terminal:exit`. The slot is mutated only by command handlers under the
//! lock; the pump only flips the stream's atomics, so there is no slot race.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Mutex;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use kodabi_core::terminal;

use crate::user_errors::reported;

/// Live PTY output, base64-encoded raw bytes (a per-chunk UTF-8 decode would
/// corrupt multibyte sequences split across reads). Mirrors `OutputPayload`.
pub const TERMINAL_OUTPUT_EVENT: &str = "terminal:output";
/// The hosted `claude` process exited (naturally — a restart/app-exit reap is
/// silent). Mirrors `ExitPayload`.
pub const TERMINAL_EXIT_EVENT: &str = "terminal:exit";

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
/// A poisoned lock means a previous holder panicked. Unlike the guards in
/// `audio_cmds`, this one does not recover with `into_inner()`, and a poisoned
/// `Mutex` stays poisoned for the life of the process: Restart re-enters
/// `terminal_restart`, which takes the same lock and fails the same way. So the
/// copy names the only thing that does work. The raw condition is a Rust fact
/// with no user-visible cause, so it is not logged through `user_errors` (there
/// is nothing to log beyond this sentence).
const POISONED: &str = "The terminal hit an internal error. Restart Kodabi to continue.";

/// The two ways a live session stops responding: the writer or the PTY resize
/// failed. Both mean the pane is talking to a process that is no longer there.
const TERMINAL_DISCONNECTED: &str =
    "The terminal lost its connection to Claude Code. Press Restart to try again.";

/// Spawning failed. Naming `claude` is the actionable half: the usual cause is
/// that it isn't installed or isn't on PATH.
const TERMINAL_START_FAILED: &str =
    "Couldn't start Claude Code. Check that the claude command is installed, then press Restart to try again.";

/// The one live terminal session, or none. Managed at builder level like
/// `CaptureState`. The `Mutex` makes the whole thing `Sync`; the session itself
/// only needs to be `Send`.
#[derive(Default)]
pub struct TerminalState(pub Mutex<Option<TerminalSession>>);

/// A running PTY hosting `claude`. The reader/coalescer threads and the child
/// handle live outside this struct (in the coalescer thread); what stays here is
/// what the command handlers need: the master (resize + keep-alive), the writer
/// (stdin), the pid (reap), and the shared stream state (the scrollback ring for
/// reattach, plus the liveness atomics the pump flips).
pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pid: Option<u32>,
    stream: terminal::SessionStream,
    cols: u16,
    rows: u16,
}

impl TerminalSession {
    fn snapshot(&self) -> TerminalSnapshot {
        TerminalSnapshot {
            running: self.stream.is_alive(),
            scrollback: BASE64.encode(self.stream.contents()),
            cols: self.cols,
            rows: self.rows,
        }
    }

    /// Deliberate teardown: mark it reaped (so the coalescer stays quiet) and
    /// kill the whole `cmd.exe → claude → kodabi-mcp` tree.
    fn reap(&self) {
        self.stream.mark_reaped();
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

/// Wraps one coalesced batch for the wire. Base64 because the payload is raw
/// PTY bytes: a per-batch UTF-8 decode would corrupt a multibyte sequence split
/// across reads, so `useXterm.ts` decodes these bytes itself.
fn output_payload(pending: &[u8]) -> OutputPayload {
    OutputPayload {
        data: BASE64.encode(pending),
    }
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
        if session.stream.is_alive() {
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
        if session.stream.is_alive() {
            session
                .writer
                .write_all(data.as_bytes())
                .map_err(|err| reported("terminal_write", err, TERMINAL_DISCONNECTED))?;
            session
                .writer
                .flush()
                .map_err(|err| reported("terminal_write", err, TERMINAL_DISCONNECTED))?;
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
            .map_err(|err| reported("terminal_resize", err, TERMINAL_DISCONNECTED))?;
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
///
/// Second caller: `updater_cmds::updater_prepare_install`, because the updater
/// exits the process from inside `Update::install()` and never reaches that
/// hook. Idempotent, so the two paths cannot double-reap.
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
    // Pre-flight, because this is the one `claude` launch whose spawn cannot
    // report a missing binary: on Windows `base_command` goes through
    // `cmd.exe /C`, which spawns fine and then prints a shell error into the
    // PTY, so the view would show "exited" over raw scrollback. Checking PATH
    // first turns that into a rejection carrying the prerequisite message, on
    // every platform alike.
    if !kodabi_llm::program_resolves(&claude_program()) {
        return Err(kodabi_core::llm::CLAUDE_MISSING_MESSAGE.to_owned());
    }

    let (mcp_path, settings_path) = write_config_files(app)?;
    let kb_root = crate::transcribe::knowledge_base_dir(app)?;

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| reported("terminal_spawn", err, TERMINAL_START_FAILED))?;

    let command = build_command(&mcp_path, &settings_path, &kb_root);
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|err| reported("terminal_spawn", err, TERMINAL_START_FAILED))?;
    let pid = child.process_id();
    // The slave is only needed to spawn against; dropping it now means the PTY
    // reports EOF once the child exits.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|err| reported("terminal_spawn", err, TERMINAL_START_FAILED))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|err| reported("terminal_spawn", err, TERMINAL_START_FAILED))?;
    let master = pair.master;

    let stream = terminal::SessionStream::new();

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || terminal::pump_reader(reader, tx));

    let coalesce_app = app.clone();
    let coalesce_stream = stream.clone();
    std::thread::spawn(move || {
        let mut child = child;
        terminal::run_coalescer(
            rx,
            terminal::StreamLimits::default(),
            &coalesce_stream,
            move || child.wait().ok().map(|status| status.exit_code() as i32),
            |pending| {
                let _ = coalesce_app.emit(TERMINAL_OUTPUT_EVENT, output_payload(pending));
            },
            |code| {
                let _ = coalesce_app.emit(TERMINAL_EXIT_EVENT, ExitPayload { code });
            },
        );
    });

    Ok(TerminalSession {
        master,
        writer,
        pid,
        stream,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    })
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
/// Every path comes from the app's own resolvers rather than from
/// `app_config_dir()`/`app_data_dir()` inline, so the sidecar is handed the same
/// vault, the same index and the same ledger this process opened — under the
/// dev sandbox that means the sidecar lands in the sandbox too (`crate::sandbox`).
/// The three state seams move together or not at all (see
/// `index_state::index_db_path` and `ledger_state::ledger_db_path`); resolving
/// one here and hard-coding another would split the sidecar's reads from its
/// writes, or let it close commitments in a different ledger than the one the
/// Commitments view shows.
///
/// The aging thresholds ride along for a different reason: they are the user's
/// settings, and the sidecar has no path to `settings.toml` and no business
/// parsing it. Passing them keeps a commitment's tier reading the same in chat
/// as in the app. Written on every open, so a change in Settings reaches the
/// sidecar the next time a terminal or chat session starts.
pub(crate) fn write_mcp_config(app: &AppHandle) -> Result<PathBuf, String> {
    let mcp_binary = resolve_mcp_binary(app)?;
    let config_dir = crate::sandbox::config_dir(app)?;
    let index_db = crate::index_state::index_db_path(app)?;
    let kb_root = crate::transcribe::knowledge_base_dir(app)?;
    let ledger_db = crate::ledger_state::ledger_db_path(app)?;
    remove_legacy_wiring(&config_dir);

    let paths = terminal::McpPaths {
        mcp_binary,
        index_db,
        kb_root,
        ledger_db,
        aging: crate::ledger_cmds::aging_config(app),
    };
    let mcp_json = terminal::mcp_config_json(&paths)
        .map_err(|err| reported("terminal_wiring", err, TERMINAL_START_FAILED))?;

    let wiring_dir = config_dir.join(WIRING_DIR);
    fs::create_dir_all(&wiring_dir)
        .map_err(|err| reported("terminal_wiring", err, TERMINAL_START_FAILED))?;
    let mcp_path = wiring_dir.join("kodabi.mcp.json");
    fs::write(&mcp_path, mcp_json)
        .map_err(|err| reported("terminal_wiring", err, TERMINAL_START_FAILED))?;
    Ok(mcp_path)
}

/// The terminal's config pair: the shared `.mcp.json` plus the Claude Code
/// settings file pre-approving the read tools (the chat spawn passes its
/// allow-list as argv instead, so the settings file stays terminal-only).
fn write_config_files(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let mcp_path = write_mcp_config(app)?;
    let config_dir = crate::sandbox::config_dir(app)?;

    let settings_json = terminal::settings_json()
        .map_err(|err| reported("terminal_wiring", err, TERMINAL_START_FAILED))?;
    let settings_path = config_dir.join(WIRING_DIR).join("terminal-settings.json");
    fs::write(&settings_path, settings_json)
        .map_err(|err| reported("terminal_wiring", err, TERMINAL_START_FAILED))?;
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
    // The developer half of this (build it, or point the env var at it) is real
    // advice, but only in a dev tree: keep it in the log, where a developer is
    // already looking, and tell an installed user the thing they can act on.
    Err(reported(
        "resolve_mcp_binary",
        format!(
            "could not locate {MCP_BINARY_NAME}; build it (cargo build -p kodabi-mcp) or set KODABI_MCP_BINARY"
        ),
        "Kodabi's helper program (kodabi-mcp) is missing from this install. Reinstall Kodabi to \
         restore it.",
    ))
}

#[cfg(windows)]
const MCP_BINARY_NAME: &str = "kodabi-mcp.exe";
#[cfg(not(windows))]
const MCP_BINARY_NAME: &str = "kodabi-mcp";

#[cfg(test)]
mod tests {
    use super::*;

    /// The three payload shapes below mirror `src/terminal.ts`; the byte pump
    /// itself is tested in `kodabi_core::terminal`.
    #[test]
    fn output_payload_is_standard_padded_base64() {
        // The standard alphabet's two distinguishing characters (`+` and `/`,
        // where the URL-safe alphabet has `-` and `_`) plus `=` padding, which
        // is what `base64ToBytes` in `src/useXterm.ts` decodes with `atob`.
        assert_eq!(output_payload(&[0xfb, 0xef, 0xbe]).data, "++++");
        assert_eq!(output_payload(&[0xff]).data, "/w==");

        let value = serde_json::to_value(output_payload(b"hi")).expect("serializes");
        assert_eq!(value, serde_json::json!({ "data": "aGk=" }));
    }

    #[test]
    fn exit_payload_wire_shape() {
        // `code` is nullable on the wire: an unknown status must not be dropped
        // to a number the frontend would read as a clean exit.
        let unknown = serde_json::to_value(ExitPayload { code: None }).expect("serializes");
        assert_eq!(unknown, serde_json::json!({ "code": null }));

        let clean = serde_json::to_value(ExitPayload { code: Some(0) }).expect("serializes");
        assert_eq!(clean, serde_json::json!({ "code": 0 }));
    }

    #[test]
    fn terminal_snapshot_wire_shape() {
        let snapshot = TerminalSnapshot {
            running: true,
            // Empty means "nothing to replay" — see `useXterm.ts`.
            scrollback: String::new(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        };
        let value = serde_json::to_value(snapshot).expect("serializes");

        assert_eq!(
            value,
            serde_json::json!({
                "running": true,
                "scrollback": "",
                "cols": 80,
                "rows": 24,
            })
        );
    }

    /// The PTY plumbing the core tests fake: a real child's bytes cross a real
    /// master, get batched by the coalescer, and land in both the sink and the
    /// replay ring. Uses `cmd.exe` rather than `claude` so it needs no install
    /// and no `KODABI_CLAUDE_BINARY` (which is process-global and would race the
    /// rest of the suite).
    ///
    /// The exit half is deliberately not asserted here, because on Windows it
    /// does not happen: the ConPTY keeps the master's read handle open after the
    /// child exits, so [`terminal::pump_reader`] never sees EOF, the channel
    /// never disconnects, and `terminal:exit` never fires (measured — the child
    /// exits, `alive` stays true, and nothing is reported). That is a real defect
    /// in the terminal's exit affordance rather than a test artifact, and fixing
    /// it is out of this change's scope; the exit and reap-suppression logic is
    /// covered in `kodabi_core::terminal` against an injected `wait`.
    #[cfg(windows)]
    #[test]
    fn a_real_pty_child_streams_its_output() {
        use std::time::Duration;

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("opens a pty");

        let shell = std::env::var_os("ComSpec").unwrap_or_else(|| OsString::from("cmd.exe"));
        let mut command = CommandBuilder::new(shell);
        command.arg("/C");
        command.arg("echo");
        command.arg("kodabi-pty-smoke");
        let mut child = pair.slave.spawn_command(command).expect("spawns the child");
        drop(pair.slave);

        // The ConPTY opens by asking the terminal where the cursor is (`ESC[6n`)
        // and waits for the report before it forwards the child's output. In the
        // app xterm.js answers; here the test does, once, up front.
        let mut writer = pair.master.take_writer().expect("takes the writer");
        writer
            .write_all(b"\x1b[1;1R")
            .expect("answers the cursor-position probe");
        writer.flush().expect("flushes the probe answer");

        let reader = pair.master.try_clone_reader().expect("clones the reader");
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || terminal::pump_reader(reader, tx));

        let stream = terminal::SessionStream::new();
        let pump_stream = stream.clone();
        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            terminal::run_coalescer(
                rx,
                terminal::StreamLimits::default(),
                &pump_stream,
                move || child.wait().ok().map(|status| status.exit_code() as i32),
                |batch| {
                    let _ = output_tx.send(batch.to_vec());
                },
                |_| {},
            );
        });

        // Drain until the child's line shows up, on a bounded deadline so a
        // wedged ConPTY fails the test instead of hanging. The first batches are
        // the ConPTY's own handshake (a cursor-position probe), so this waits
        // for the marker rather than for the stream to go quiet.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut output: Vec<u8> = Vec::new();
        while !String::from_utf8_lossy(&output).contains("kodabi-pty-smoke") {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .unwrap_or_default();
            let batch = output_rx.recv_timeout(remaining).unwrap_or_else(|_| {
                panic!("timed out; got {:?}", String::from_utf8_lossy(&output))
            });
            output.extend_from_slice(&batch);
        }
        // A batch reaches the sink only after the ring has taken it, so the ring
        // holds everything received here — and possibly a later batch too.
        assert!(
            stream.contents().starts_with(&output),
            "the ring replays what was emitted"
        );

        // The threads block on a master the ConPTY holds open; dropping it here
        // releases what this test owns, and the harness reaps the rest.
        drop(pair.master);
    }
}
