//! Kodabi's headless Claude Code runner.
//!
//! The one place in the app that spawns `claude` as a subprocess — the
//! MCP-inversion loop's subprocess boundary (FOUNDING_DOC §3.2, §3.4). Pure
//! prompt-building and response-parsing logic lives in `kodabi-core` behind
//! `kodabi_core::llm::HeadlessClaude` (the glossary cleanup pass in
//! `transcription::cleanup`, the end-of-meeting distill in `distill`); this
//! crate only owns the side-effecting process spawn, so the rest of the app
//! stays engine- and process-agnostic and unit-testable against a mock.
//!
//! Auth is never this crate's concern: a headless `claude -p` run reuses
//! whatever the machine's Claude Code is already authenticated with — the
//! user's subscription login or `ANTHROPIC_API_KEY`. The flags in [`invoke`]
//! disable tools/MCP/prompts without ever forcing API-key-only auth (unlike
//! e.g. `--bare`, which is deliberately never used here since it would
//! silently break subscription login).
//!
//! On Windows, an npm-installed `claude` is typically a `.cmd` shim, and that
//! is the one install shape this crate cannot spawn: the PATH search behind
//! `std::process::Command` appends only `.exe` to a bare name, so a machine
//! whose only `claude` is `claude.cmd` fails here with `ErrorKind::NotFound`
//! while having Claude Code installed. (Rust >= 1.77.2 does *invoke* a
//! `.cmd`/`.bat` safely, shelling it through `cmd.exe` with correct escaping —
//! the fix for CVE-2024-24576 — but that is about invocation once the name has
//! resolved, not about resolving a bare name to it. The embedded terminal hits
//! neither problem: it runs `cmd.exe /C claude`, which honours `PATHEXT`.)
//!
//! No manual `cmd /C` wrapping here, so on such a machine the headless passes
//! fail. What must not follow is telling the user Claude Code is missing when
//! it is not — see [`is_missing_claude`], which confirms absence against
//! `PATHEXT` before the prerequisite message is used.

pub mod chat;

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use kodabi_core::llm::{HeadlessClaude, LlmRequest, LlmRunError};

/// The glossary cleanup pass's model: the cheapest alias capable of that
/// narrow task; Claude Code resolves it to the latest Haiku snapshot.
/// Override via [`ClaudeConfig::model`] or the `KODABI_CLEANUP_MODEL` env
/// var ([`ClaudeConfig::cleanup_from_env`]).
pub const DEFAULT_MODEL: &str = "haiku";

/// Default cap on a single cleanup run. Long meeting transcripts can take a
/// while for the cheap model to chew through, so this is generous — and
/// overridable via [`ClaudeConfig::timeout`] or the `KODABI_CLEANUP_TIMEOUT_SECS`
/// env var ([`ClaudeConfig::cleanup_from_env`]) when even this is too tight.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// The distill pass's model. Summarization + extraction quality shapes the
/// note the knowledge base keeps forever, so this defaults a tier above the
/// cleanup pass's. Override via `KODABI_DISTILL_MODEL`
/// ([`ClaudeConfig::distill_from_env`]).
pub const DISTILL_DEFAULT_MODEL: &str = "sonnet";

/// Default cap on a single distill run: a whole-meeting pass on a stronger
/// model, so far roomier than the cleanup default. Override via
/// `KODABI_DISTILL_TIMEOUT_SECS` ([`ClaudeConfig::distill_from_env`]).
///
/// This is a **per-call** cap, not a per-session one: a transcript over
/// `kodabi_core::distill`'s input budget is chunked into one call per chunk
/// plus a merge call, and each of them gets this timeout in full.
pub const DISTILL_DEFAULT_TIMEOUT_SECS: u64 = 180;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(DEFAULT_TIMEOUT_SECS);

/// Configuration for a headless `claude` invocation.
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// Model alias or full ID passed to `--model`.
    pub model: String,
    /// Kill the subprocess and fail if it hasn't exited by this long.
    pub timeout: Duration,
    /// Explicit path to the `claude` binary. `None` resolves the bare name
    /// `claude` via `PATH` (see the module docs on Windows `.cmd` shims).
    pub binary: Option<PathBuf>,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL.to_owned(),
            timeout: DEFAULT_TIMEOUT,
            binary: None,
        }
    }
}

impl ClaudeConfig {
    /// The glossary cleanup pass's config: [`ClaudeConfig::default`] with
    /// `KODABI_CLEANUP_MODEL` and `KODABI_CLEANUP_TIMEOUT_SECS` overrides
    /// applied, if set and valid.
    pub fn cleanup_from_env() -> Self {
        let config =
            apply_model_override(Self::default(), std::env::var("KODABI_CLEANUP_MODEL").ok());
        apply_timeout_override(config, std::env::var("KODABI_CLEANUP_TIMEOUT_SECS").ok())
    }

    /// The distill pass's base config: [`DISTILL_DEFAULT_MODEL`] and
    /// [`DISTILL_DEFAULT_TIMEOUT_SECS`], no explicit binary.
    pub fn distill() -> Self {
        Self {
            model: DISTILL_DEFAULT_MODEL.to_owned(),
            timeout: Duration::from_secs(DISTILL_DEFAULT_TIMEOUT_SECS),
            binary: None,
        }
    }

    /// [`ClaudeConfig::distill`] with `KODABI_DISTILL_MODEL` and
    /// `KODABI_DISTILL_TIMEOUT_SECS` overrides applied, if set and valid.
    pub fn distill_from_env() -> Self {
        let config =
            apply_model_override(Self::distill(), std::env::var("KODABI_DISTILL_MODEL").ok());
        apply_timeout_override(config, std::env::var("KODABI_DISTILL_TIMEOUT_SECS").ok())
    }
}

fn apply_model_override(mut config: ClaudeConfig, model: Option<String>) -> ClaudeConfig {
    if let Some(model) = model {
        if !model.trim().is_empty() {
            config.model = model;
        }
    }
    config
}

fn apply_timeout_override(mut config: ClaudeConfig, secs: Option<String>) -> ClaudeConfig {
    if let Some(secs) = secs {
        // Ignore blank or non-positive/garbage values: a zero timeout would
        // kill every run before it started, so a bad override falls back to
        // the default rather than silently disabling the pass.
        if let Ok(secs) = secs.trim().parse::<u64>() {
            if secs > 0 {
                config.timeout = Duration::from_secs(secs);
            }
        }
    }
    config
}

/// Runs any headless pass through a real `claude` subprocess. Implements
/// [`HeadlessClaude`] so one instance plugs directly into
/// `kodabi_core::transcription::clean_transcript` (built with
/// [`ClaudeConfig::cleanup_from_env`]) or `kodabi_core::distill::distill_session`
/// (built with [`ClaudeConfig::distill_from_env`]) — the pass's character
/// lives entirely in its config and request.
pub struct ClaudeRunner {
    config: ClaudeConfig,
}

impl ClaudeRunner {
    pub fn new(config: ClaudeConfig) -> Self {
        Self { config }
    }
}

impl HeadlessClaude for ClaudeRunner {
    fn run(&self, request: &LlmRequest) -> Result<String, LlmRunError> {
        let raw = invoke(&self.config, request)?;
        parse_envelope(&raw)
    }
}

/// Spawns `claude`, writes `request.prompt` to its stdin, and returns raw
/// stdout. Parsing the JSON envelope is [`parse_envelope`]'s job, kept
/// separate so it can be unit-tested without a real subprocess.
///
/// Hermetic by construction: no tools (`--tools ""`), no MCP servers
/// (`--strict-mcp-config` with an empty `--mcp-config`), no project/user
/// settings (`--setting-sources ""`), and `--permission-mode dontAsk` so a
/// denied action fails immediately instead of blocking on a prompt with no
/// TTY to answer it. Every flag here was verified against a live `claude`
/// invocation, including that this combination preserves subscription auth.
///
/// `CLAUDE_CODE_SKIP_PROMPT_HISTORY` disables Claude Code's own session-log and
/// prompt-history writing for this run. That closes the retention gap
/// FOUNDING_DOC §3.7 names: the transcript text passed here would otherwise
/// linger in `~/.claude`, outside the in-app retention policy's reach. Scoped to
/// this spawn's environment, so it never touches the user's global config.
fn invoke(config: &ClaudeConfig, request: &LlmRequest) -> Result<String, LlmRunError> {
    let program = config
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("claude"));

    let mut command = hidden_command(program.clone());
    command
        .arg("-p")
        .arg("--output-format")
        .arg("json")
        .arg("--model")
        .arg(&config.model)
        .arg("--system-prompt")
        .arg(&request.system_prompt)
        .arg("--tools")
        .arg("")
        .arg("--strict-mcp-config")
        .arg("--mcp-config")
        .arg(r#"{"mcpServers":{}}"#)
        .arg("--setting-sources")
        .arg("")
        .arg("--permission-mode")
        .arg("dontAsk")
        // Disable Claude Code's own transcript/prompt-history logging for this
        // run (FOUNDING_DOC §3.7). See the fn doc.
        .env("CLAUDE_CODE_SKIP_PROMPT_HISTORY", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // A missing `claude` is the one spawn failure with a remedy the user can
    // act on, and it is what a fresh install hits first (the CLI is a
    // user-installed prerequisite), so it gets its own variant here rather
    // than reaching the toast as an OS error string. Only *this* spawn site
    // maps it: the other `Spawn` producers below (empty stdout, wait failure,
    // timeout) are all cases where the binary was found and ran.
    let child = command.spawn().map_err(|err| {
        if is_missing_claude(&err, &program) {
            LlmRunError::ClaudeMissing
        } else {
            LlmRunError::Spawn(err.to_string())
        }
    })?;

    let output = wait_with_timeout(child, &request.prompt, config.timeout)?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !stdout.trim().is_empty() {
        return Ok(stdout);
    }

    // No JSON on stdout at all: the process failed before it could produce
    // anything the envelope contract covers (e.g. spawned but crashed
    // immediately).
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(LlmRunError::Spawn(stderr.trim().to_owned()))
}

/// Feeds `prompt` to `child`, waits for it to exit, and returns its captured
/// output — killing it if `timeout` elapses first.
///
/// No async runtime in this codebase (house style favors `std::thread` +
/// channels — see `src-tauri/src/capture_control.rs`), so this offloads both
/// the blocking stdin write and the blocking `wait_with_output` to threads
/// and races them against `recv_timeout`. Crucially the stdin write lives on
/// its own thread: a prompt larger than the OS pipe buffer would otherwise
/// deadlock against claude's own stdout/stderr writes, and — because it runs
/// before the wait — escape the timeout entirely, hanging the caller forever.
/// A failed write (e.g. claude exited early) is ignored here so the real
/// diagnostic on stderr survives in the returned [`Output`].
fn wait_with_timeout(
    mut child: Child,
    prompt: &str,
    timeout: Duration,
) -> Result<Output, LlmRunError> {
    let pid = child.id();

    if let Some(mut stdin) = child.stdin.take() {
        let prompt = prompt.to_owned();
        std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
            // `stdin` drops here, closing the pipe so claude sees EOF.
        });
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(err)) => Err(LlmRunError::Spawn(err.to_string())),
        Err(_) => {
            kill_process_tree(pid);
            Err(LlmRunError::Spawn(format!(
                "headless Claude Code did not exit within {timeout:?}"
            )))
        }
    }
}

/// Kills a process and its whole descendant tree, best-effort and quiet (no
/// console window pops). On Windows that is `taskkill /T /F`, which reaps the
/// subtree — exactly what the embedded terminal needs to reap
/// `claude → kodabi-mcp` on app exit, so `src-tauri`'s `terminal_cmds` reuses
/// this rather than duplicating the platform split.
#[cfg(windows)]
pub fn kill_process_tree(pid: u32) {
    let _ = hidden_command("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

/// See the Windows variant. `kill -9` on the pid (no tree flag; the callers here
/// spawn no descendants worth reaping on non-Windows).
#[cfg(not(windows))]
pub fn kill_process_tree(pid: u32) {
    let _ = hidden_command("kill")
        .arg("-9")
        .arg(pid.to_string())
        .output();
}

/// `CREATE_NO_WINDOW` (Win32 process-creation flag): the child gets a console
/// but no console window. Kodabi's release build is a GUI-subsystem app with
/// no console of its own, so without this every spawned console process
/// (`claude`, `taskkill`) pops a visible terminal on the user's screen.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `Command::new` that never pops a console window on Windows. Every spawn in
/// this crate goes through this constructor rather than `Command::new`
/// directly.
///
/// The platform split lives in `hide_console_window` (mirroring
/// [`kill_process_tree`]) rather than an inline `#[cfg(windows)]` block, so the
/// non-Windows expansion doesn't strip every use of the binding and trip
/// `unused_mut`/`let_and_return` under `-D warnings`.
fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    hide_console_window(&mut command);
    command
}

#[cfg(windows)]
fn hide_console_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console_window(_command: &mut Command) {}

/// Whether the spawn failure `err` means Claude Code is genuinely absent, and
/// so may be reported with `CLAUDE_MISSING_MESSAGE`.
///
/// `ErrorKind::NotFound` is necessary but **not sufficient**, which is the
/// whole reason this exists: per the module docs, a bare `claude` that only
/// exists as an npm `claude.cmd` fails that way on Windows while being
/// installed. Claiming "Claude Code isn't installed" there would be flatly
/// wrong — the same machine runs `claude` in the embedded terminal — so
/// absence is confirmed against `PATHEXT` via [`program_resolves`] before the
/// prerequisite message is used, and anything else stays the OS error it is.
pub(crate) fn is_missing_claude(err: &std::io::Error, program: &std::path::Path) -> bool {
    err.kind() == std::io::ErrorKind::NotFound && !program_resolves(program.as_os_str())
}

/// Whether `program` would resolve at spawn time, reading `PATH` (and, on
/// Windows, `PATHEXT`) from the environment.
///
/// Two callers, for two different reasons. The embedded terminal **cannot**
/// learn this from its spawn at all: it launches through `cmd.exe /C claude`
/// on Windows (portable-pty's `CommandBuilder` executes the program directly
/// and so cannot run a `.cmd` shim), so a missing `claude` spawns `cmd.exe`
/// *successfully* and only surfaces as a shell error inside the PTY. The spawn
/// sites in this crate do get `ErrorKind::NotFound` directly, but that error
/// over-reports absence on Windows, so they read this through
/// [`is_missing_claude`] to tell the two apart.
pub fn program_resolves(program: &std::ffi::OsStr) -> bool {
    let dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    resolve_program(program, &dirs, &path_extensions()).is_some()
}

/// The suffixes a bare program name may carry. `""` (the name as given) always
/// leads; on Windows `PATHEXT` supplies the rest, since an npm-installed
/// `claude` is a `claude.cmd` that `PATH` alone never matches.
fn path_extensions() -> Vec<String> {
    let mut extensions = vec![String::new()];
    if cfg!(windows) {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
        extensions.extend(
            raw.split(';')
                .map(str::trim)
                .filter(|ext| !ext.is_empty())
                .map(str::to_owned),
        );
    }
    extensions
}

/// The pure half of [`program_resolves`]: looks `program` up against explicit
/// `dirs` and `exts` instead of the environment, so it is testable anywhere.
///
/// A name carrying a separator (`C:\tools\claude.exe`, `./claude`) is an
/// explicit path and is checked as one, never searched — matching how the OS
/// treats it at spawn. Existence, not executability, is the test: this is a
/// pre-flight, and the real spawn stays the authority on whether the file runs.
pub fn resolve_program(
    program: &std::ffi::OsStr,
    dirs: &[PathBuf],
    exts: &[String],
) -> Option<PathBuf> {
    let as_path = std::path::Path::new(program);
    if as_path.components().count() > 1 {
        return first_existing(as_path, exts);
    }
    dirs.iter()
        .find_map(|dir| first_existing(&dir.join(as_path), exts))
}

/// The first of `candidate` + each extension that exists as a file.
fn first_existing(candidate: &std::path::Path, exts: &[String]) -> Option<PathBuf> {
    exts.iter().find_map(|ext| {
        let path = if ext.is_empty() {
            candidate.to_path_buf()
        } else {
            let mut name = candidate.as_os_str().to_os_string();
            name.push(ext);
            PathBuf::from(name)
        };
        path.is_file().then_some(path)
    })
}

/// Envelope shape from `claude -p --output-format json`. On failure, the
/// message lands in `result` itself (there is no separate `error` field);
/// `is_error` is what distinguishes the two cases — verified against a live
/// failing invocation, not assumed from docs.
#[derive(Debug, serde::Deserialize)]
struct Envelope {
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    result: Option<String>,
}

/// Parses `claude -p --output-format json`'s stdout and extracts the model's
/// text result, or a [`LlmRunError`] if Claude reported a failure or the
/// envelope carries no usable text.
fn parse_envelope(raw: &str) -> Result<String, LlmRunError> {
    let envelope: Envelope = serde_json::from_str(raw.trim())
        .map_err(|err| LlmRunError::ClaudeError(format!("unparsable output: {err}")))?;
    let text = envelope.result.unwrap_or_default();

    if envelope.is_error {
        return Err(LlmRunError::ClaudeError(if text.trim().is_empty() {
            "unknown error".to_owned()
        } else {
            text
        }));
    }

    if text.trim().is_empty() {
        return Err(LlmRunError::EmptyResult);
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_uses_the_cheap_model() {
        assert_eq!(ClaudeConfig::default().model, DEFAULT_MODEL);
    }

    #[test]
    fn distill_config_uses_the_stronger_model_and_longer_timeout() {
        let config = ClaudeConfig::distill();

        assert_eq!(config.model, DISTILL_DEFAULT_MODEL);
        assert_eq!(
            config.timeout,
            Duration::from_secs(DISTILL_DEFAULT_TIMEOUT_SECS)
        );
        assert!(config.binary.is_none());
    }

    #[test]
    fn distill_overrides_apply_on_top_of_the_distill_defaults() {
        // `distill_from_env` composes `distill()` with the same override
        // helpers tested above; this pins that the base really is the
        // distill config, not `default()`.
        let config = apply_timeout_override(ClaudeConfig::distill(), Some("  ".to_owned()));

        assert_eq!(
            config.timeout,
            Duration::from_secs(DISTILL_DEFAULT_TIMEOUT_SECS)
        );
    }

    #[test]
    fn model_override_replaces_the_default() {
        let config =
            apply_model_override(ClaudeConfig::default(), Some("claude-opus-4-8".to_owned()));

        assert_eq!(config.model, "claude-opus-4-8");
    }

    #[test]
    fn model_override_ignores_a_blank_value() {
        let config = apply_model_override(ClaudeConfig::default(), Some("   ".to_owned()));

        assert_eq!(config.model, DEFAULT_MODEL);
    }

    #[test]
    fn model_override_ignores_a_missing_value() {
        let config = apply_model_override(ClaudeConfig::default(), None);

        assert_eq!(config.model, DEFAULT_MODEL);
    }

    #[test]
    fn timeout_override_replaces_the_default() {
        let config = apply_timeout_override(ClaudeConfig::default(), Some("120".to_owned()));

        assert_eq!(config.timeout, Duration::from_secs(120));
    }

    #[test]
    fn timeout_override_ignores_zero_and_garbage() {
        for bad in ["0", "", "  ", "-5", "abc"] {
            let config = apply_timeout_override(ClaudeConfig::default(), Some(bad.to_owned()));
            assert_eq!(
                config.timeout, DEFAULT_TIMEOUT,
                "input {bad:?} should be ignored"
            );
        }
    }

    #[test]
    fn timeout_override_ignores_a_missing_value() {
        let config = apply_timeout_override(ClaudeConfig::default(), None);

        assert_eq!(config.timeout, DEFAULT_TIMEOUT);
    }

    #[test]
    fn parses_a_successful_envelope() {
        let raw = r#"{"is_error":false,"result":"[{\"index\":0,\"text\":\"MERIDIAN\"}]"}"#;

        let result = parse_envelope(raw).expect("should parse");

        assert_eq!(result, r#"[{"index":0,"text":"MERIDIAN"}]"#);
    }

    #[test]
    fn surfaces_an_error_reported_in_the_result_field() {
        let raw = r#"{"is_error":true,"result":"model not found"}"#;

        let err = parse_envelope(raw).unwrap_err();

        match err {
            LlmRunError::ClaudeError(message) => assert_eq!(message, "model not found"),
            other => panic!("expected ClaudeError, got {other:?}"),
        }
    }

    #[test]
    fn is_error_with_no_result_text_falls_back_to_a_generic_message() {
        let raw = r#"{"is_error":true}"#;

        let err = parse_envelope(raw).unwrap_err();

        assert!(matches!(err, LlmRunError::ClaudeError(_)));
    }

    #[test]
    fn empty_result_is_an_error() {
        let raw = r#"{"is_error":false,"result":""}"#;

        let err = parse_envelope(raw).unwrap_err();

        assert!(matches!(err, LlmRunError::EmptyResult));
    }

    #[test]
    fn unparsable_json_is_an_error() {
        let err = parse_envelope("not json").unwrap_err();

        assert!(matches!(err, LlmRunError::ClaudeError(_)));
    }

    #[test]
    fn a_missing_binary_is_reported_as_the_prerequisite_it_is() {
        // An explicit path that cannot exist yields `ErrorKind::NotFound` on
        // every platform, which is the same error a bare `claude` missing from
        // PATH produces.
        let dir = tempfile::tempdir().expect("tempdir");
        let config = ClaudeConfig {
            binary: Some(dir.path().join("no-such-claude")),
            ..ClaudeConfig::default()
        };

        let err = invoke(
            &config,
            &LlmRequest {
                system_prompt: "sys".to_owned(),
                prompt: "hi".to_owned(),
            },
        )
        .unwrap_err();

        assert!(matches!(err, LlmRunError::ClaudeMissing), "got {err:?}");
    }

    #[test]
    fn a_not_found_spawn_of_a_program_that_exists_is_not_called_missing() {
        // The npm-shim case, which is the reason `NotFound` alone can't decide
        // this: on Windows the PATH search behind `std::process::Command`
        // appends only `.exe`, so a machine whose only `claude` is a `.cmd`
        // gets `NotFound` while Claude Code is installed and the embedded
        // terminal runs it. Telling that user to go install it is worse than
        // the OS error was.
        let dir = tempfile::tempdir().expect("tempdir");
        let shim = dir.path().join("claude.cmd");
        std::fs::write(&shim, "").expect("write");
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "program not found");

        assert!(
            !is_missing_claude(&not_found, &shim),
            "a program on disk is not a missing prerequisite"
        );
        assert!(is_missing_claude(
            &not_found,
            &dir.path().join("no-such-claude")
        ));
    }

    #[test]
    fn a_spawn_failure_that_is_not_not_found_is_never_called_missing() {
        // Permission denied, ENOEXEC, a full process table: the binary was
        // found, so whatever went wrong, it wasn't absent.
        let dir = tempfile::tempdir().expect("tempdir");
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");

        assert!(!is_missing_claude(
            &denied,
            &dir.path().join("no-such-claude")
        ));
    }

    #[test]
    fn a_bare_name_resolves_through_a_path_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("claude"), "").expect("write");

        let found = resolve_program(
            std::ffi::OsStr::new("claude"),
            &[dir.path().to_path_buf()],
            &[String::new()],
        );

        assert_eq!(found, Some(dir.path().join("claude")));
    }

    #[test]
    fn a_bare_name_resolves_through_an_extension() {
        // The npm-installed Windows case: only `claude.cmd` is on disk, so the
        // bare name matches nothing and the extension is what finds it.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("claude.cmd"), "").expect("write");

        let found = resolve_program(
            std::ffi::OsStr::new("claude"),
            &[dir.path().to_path_buf()],
            &[String::new(), ".CMD".to_owned()],
        )
        .expect("should resolve through the extension");

        // Compared case-insensitively: the resolved path carries the casing of
        // the extension that matched (`.CMD`), which on a case-insensitive
        // filesystem need not be the casing on disk.
        assert_eq!(
            found.to_string_lossy().to_lowercase(),
            dir.path()
                .join("claude.cmd")
                .to_string_lossy()
                .to_lowercase()
        );
    }

    #[test]
    fn a_name_absent_from_every_directory_does_not_resolve() {
        let dir = tempfile::tempdir().expect("tempdir");

        let found = resolve_program(
            std::ffi::OsStr::new("claude"),
            &[dir.path().to_path_buf()],
            &[String::new(), ".CMD".to_owned()],
        );

        assert_eq!(found, None);
    }

    #[test]
    fn an_explicit_path_is_checked_rather_than_searched() {
        // A path with separators must not be looked up in the PATH dirs, even
        // when a file of the same name sits in one of them.
        let dir = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("claude"), "").expect("write");
        let explicit = elsewhere.path().join("claude");

        let dirs = [dir.path().to_path_buf()];
        assert_eq!(
            resolve_program(explicit.as_os_str(), &dirs, &[String::new()]),
            None
        );

        std::fs::write(&explicit, "").expect("write");
        assert_eq!(
            resolve_program(explicit.as_os_str(), &dirs, &[String::new()]),
            Some(explicit)
        );
    }

    #[test]
    fn a_directory_is_not_a_resolved_program() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("claude")).expect("mkdir");

        let found = resolve_program(
            std::ffi::OsStr::new("claude"),
            &[dir.path().to_path_buf()],
            &[String::new()],
        );

        assert_eq!(found, None);
    }

    #[test]
    fn a_program_that_is_really_installed_resolves_against_the_real_path() {
        // The failure this guards is the expensive direction: the terminal
        // pre-flights with `program_resolves`, so a wrapper that read PATH or
        // PATHEXT wrongly would refuse to open a session on machines where
        // `claude` IS installed. A bare name found only through an extension
        // (`cmd` -> `cmd.exe`) exercises the same lookup `claude.cmd` needs.
        let installed = if cfg!(windows) { "cmd" } else { "sh" };

        assert!(
            program_resolves(std::ffi::OsStr::new(installed)),
            "{installed} should resolve on PATH"
        );
        assert!(!program_resolves(std::ffi::OsStr::new(
            "kodabi-no-such-program"
        )));
    }
}
