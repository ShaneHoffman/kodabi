//! Kodama's headless Claude Code runner.
//!
//! The one place in the app that spawns `claude` as a subprocess — the
//! MCP-inversion loop's subprocess boundary (FOUNDING_DOC §3.2, §3.4). Pure
//! prompt-building and response-merging logic lives in
//! `kodama_core::transcription::cleanup`; this crate only owns the
//! side-effecting process spawn, so the rest of the app stays engine- and
//! process-agnostic and unit-testable against a mock
//! `kodama_core::transcription::HeadlessClaude`.
//!
//! Auth is never this crate's concern: a headless `claude -p` run reuses
//! whatever the machine's Claude Code is already authenticated with — the
//! user's subscription login or `ANTHROPIC_API_KEY`. The flags in [`invoke`]
//! disable tools/MCP/prompts without ever forcing API-key-only auth (unlike
//! e.g. `--bare`, which is deliberately never used here since it would
//! silently break subscription login).
//!
//! On Windows, an npm-installed `claude` is typically a `.cmd` shim.
//! `std::process::Command` resolves and invokes it safely on its own
//! (Rust >= 1.77.2 shells `.cmd`/`.bat` targets through `cmd.exe` internally
//! with correct escaping — the fix for CVE-2024-24576), so no manual
//! `cmd /C` wrapping is needed here.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use kodama_core::transcription::{CleanupRequest, CleanupRunError, HeadlessClaude};

/// Cheapest model alias capable of this task; Claude Code resolves it to the
/// latest Haiku snapshot. Override via [`ClaudeConfig::model`] or the
/// `KODAMA_CLEANUP_MODEL` env var ([`ClaudeConfig::from_env`]).
pub const DEFAULT_MODEL: &str = "haiku";

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

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
    /// [`ClaudeConfig::default`] with a `KODAMA_CLEANUP_MODEL` override
    /// applied, if set and non-blank.
    pub fn from_env() -> Self {
        apply_model_override(Self::default(), std::env::var("KODAMA_CLEANUP_MODEL").ok())
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

/// Runs the glossary cleanup post-pass through a real headless `claude`
/// subprocess. Implements [`HeadlessClaude`] so it plugs directly into
/// `kodama_core::transcription::clean_transcript`.
pub struct ClaudeCleaner {
    config: ClaudeConfig,
}

impl ClaudeCleaner {
    pub fn new(config: ClaudeConfig) -> Self {
        Self { config }
    }
}

impl HeadlessClaude for ClaudeCleaner {
    fn run(&self, request: &CleanupRequest) -> Result<String, CleanupRunError> {
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
fn invoke(config: &ClaudeConfig, request: &CleanupRequest) -> Result<String, CleanupRunError> {
    let program = config
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("claude"));

    let mut command = Command::new(program);
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
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|err| CleanupRunError::Spawn(err.to_string()))?;

    write_prompt(&mut child, &request.prompt)
        .map_err(|err| CleanupRunError::Spawn(err.to_string()))?;

    let output = wait_with_timeout(child, config.timeout)?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !stdout.trim().is_empty() {
        return Ok(stdout);
    }

    // No JSON on stdout at all: the process failed before it could produce
    // anything the envelope contract covers (e.g. spawned but crashed
    // immediately).
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(CleanupRunError::Spawn(stderr.trim().to_owned()))
}

fn write_prompt(child: &mut Child, prompt: &str) -> std::io::Result<()> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::other("child stdin was not piped"))?;
    stdin.write_all(prompt.as_bytes())
}

/// Waits for `child` to exit, killing it if `timeout` elapses first.
///
/// No async runtime in this codebase (house style favors `std::thread` +
/// channels — see `src-tauri/src/capture_control.rs`), so this offloads the
/// blocking `wait_with_output` to a thread and races it against
/// `recv_timeout`.
fn wait_with_timeout(child: Child, timeout: Duration) -> Result<Output, CleanupRunError> {
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(err)) => Err(CleanupRunError::Spawn(err.to_string())),
        Err(_) => {
            kill_process(pid);
            Err(CleanupRunError::Spawn(format!(
                "headless Claude Code did not exit within {timeout:?}"
            )))
        }
    }
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
}

#[cfg(not(windows))]
fn kill_process(pid: u32) {
    let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
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
/// text result, or a [`CleanupRunError`] if Claude reported a failure or the
/// envelope carries no usable text.
fn parse_envelope(raw: &str) -> Result<String, CleanupRunError> {
    let envelope: Envelope = serde_json::from_str(raw.trim())
        .map_err(|err| CleanupRunError::ClaudeError(format!("unparsable output: {err}")))?;
    let text = envelope.result.unwrap_or_default();

    if envelope.is_error {
        return Err(CleanupRunError::ClaudeError(if text.trim().is_empty() {
            "unknown error".to_owned()
        } else {
            text
        }));
    }

    if text.trim().is_empty() {
        return Err(CleanupRunError::EmptyResult);
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
            CleanupRunError::ClaudeError(message) => assert_eq!(message, "model not found"),
            other => panic!("expected ClaudeError, got {other:?}"),
        }
    }

    #[test]
    fn is_error_with_no_result_text_falls_back_to_a_generic_message() {
        let raw = r#"{"is_error":true}"#;

        let err = parse_envelope(raw).unwrap_err();

        assert!(matches!(err, CleanupRunError::ClaudeError(_)));
    }

    #[test]
    fn empty_result_is_an_error() {
        let raw = r#"{"is_error":false,"result":""}"#;

        let err = parse_envelope(raw).unwrap_err();

        assert!(matches!(err, CleanupRunError::EmptyResult));
    }

    #[test]
    fn unparsable_json_is_an_error() {
        let err = parse_envelope("not json").unwrap_err();

        assert!(matches!(err, CleanupRunError::ClaudeError(_)));
    }
}
