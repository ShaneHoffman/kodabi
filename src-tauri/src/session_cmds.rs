//! Thin Tauri command wrappers over `kodabi_core::sessions` for the note
//! view's source pairing: resolving a distilled note's `source:` field to its
//! raw transcript and retained recording, and revealing that recording in
//! Explorer. Validation and artifact resolution live in `kodabi-core`; these
//! commands only own the serde IPC DTOs, resolve the knowledge-base root, and
//! map results to user-facing copy (see `user_errors`), with the raw detail
//! going to stderr — the same convention `note_cmds` uses.

use std::path::{Component, Path, PathBuf};

use kodabi_core::transcription::Channel;
use tauri::AppHandle;

use crate::transcribe::knowledge_base_dir;
use crate::user_errors::reported;

/// Every rejection from [`validate_audio_path`] says the same thing to the
/// reader: the press did nothing, and the recording is still where it was. The
/// specific lexical rule that failed is a fact about our own IPC, so it goes to
/// the log.
const AUDIO_PATH_REJECTED: &str = "Kodabi couldn't verify the recording's location, so it wasn't \
                                   opened. Reopen the note and try again.";

/// One transcript segment, in the shape the frontend renders. Mirrors
/// `kodabi_core::raw_session::TranscriptSegment` (and the MCP
/// `TranscriptSegment` shape in `docs/MCP_TOOL_SURFACE.md`): `channel` wires
/// as `"you" | "them" | "unknown"`, timestamps are millisecond offsets from
/// session start.
#[derive(serde::Serialize)]
pub struct TranscriptSegmentDto {
    index: u64,
    channel: Channel,
    speaker: Option<String>,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

/// What a note's session source resolves to. Mirrors
/// `kodabi_core::sessions::SessionArtifacts`; `audio_path` is absolute — the
/// exact value [`reveal_session_audio`] takes back, and what the asset
/// protocol serves for in-app playback.
#[derive(serde::Serialize)]
pub struct SessionArtifactsDto {
    transcript_available: bool,
    segments: Vec<TranscriptSegmentDto>,
    audio_path: Option<String>,
}

/// Resolves a note's `source:` value (`sessions/<file>.jsonl`) to its raw
/// transcript segments and retained recording. A retention-pruned transcript
/// is not an error: it reports `transcript_available: false` with empty
/// segments, and the recording is resolved independently.
#[tauri::command]
pub async fn read_session_artifacts(
    app: AppHandle,
    source: String,
) -> Result<SessionArtifactsDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let artifacts = kodabi_core::sessions::read_session_artifacts(&kb, &source).map_err(|err| {
        reported(
            "read_session_artifacts",
            err,
            "Couldn't read this note's session files. The note itself is fine; reopen it to try \
             again.",
        )
    })?;
    Ok(SessionArtifactsDto {
        transcript_available: artifacts.transcript_available,
        segments: artifacts
            .segments
            .into_iter()
            .map(|segment| TranscriptSegmentDto {
                index: segment.index,
                channel: segment.channel,
                speaker: segment.speaker,
                start_ms: segment.start_ms,
                end_ms: segment.end_ms,
                text: segment.text,
            })
            .collect(),
        audio_path: artifacts.audio_path.map(|path| path.display().to_string()),
    })
}

/// Opens Explorer with the retained recording selected. `audio_path` is the
/// absolute path [`read_session_artifacts`] reported; anything else — a
/// non-`.wav`, a traversal segment, a path outside `<kb>/sessions` — is
/// rejected before touching the filesystem.
#[tauri::command]
pub fn reveal_session_audio(app: AppHandle, audio_path: String) -> Result<(), String> {
    let kb = knowledge_base_dir(&app)?;
    let path = validate_audio_path(&kb, &audio_path)?;
    if !path.is_file() {
        // Retention may have discarded it since the note view resolved it.
        return Err(
            "The recording file is missing. Retention may have discarded it; the note itself is \
             unaffected."
                .to_string(),
        );
    }
    reveal_in_explorer(&path)
}

/// Validates an IPC-supplied recording path: the absolute path of a `.wav`
/// file directly inside `<kb>/sessions` — the shape [`read_session_artifacts`]
/// reports, which is where a frontend caller gets it. Purely lexical, the
/// mirror of `distill_cmds::validate_session_path` for the audio sibling.
fn validate_audio_path(kb: &Path, raw: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(raw);
    if path.extension().and_then(|ext| ext.to_str()) != Some("wav") {
        return Err(reported(
            "validate_audio_path",
            format!("not a .wav recording file: {raw}"),
            AUDIO_PATH_REJECTED,
        ));
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(reported(
            "validate_audio_path",
            format!("recording path must not contain '.' or '..' segments: {raw}"),
            AUDIO_PATH_REJECTED,
        ));
    }
    let sessions = kb.join("sessions");
    if path.parent() != Some(sessions.as_path()) {
        return Err(reported(
            "validate_audio_path",
            format!(
                "recording path must be directly inside {}",
                sessions.display()
            ),
            AUDIO_PATH_REJECTED,
        ));
    }
    Ok(path)
}

/// Launches `explorer /select,` on the validated path and returns without
/// waiting: Explorer conventionally exits nonzero even on success, so only a
/// failure to launch is an error. `raw_arg` keeps the `/select,"<path>"` form
/// intact — std's automatic quoting would wrap the whole argument and split
/// paths containing spaces.
#[cfg(windows)]
fn reveal_in_explorer(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .map_err(|err| {
            reported(
                "reveal_in_explorer",
                err,
                "Couldn't open Explorer. The recording is still in your vault's sessions folder.",
            )
        })?;
    Ok(())
}

#[cfg(not(windows))]
fn reveal_in_explorer(_path: &Path) -> Result<(), String> {
    Err("Revealing the recording is only supported on Windows.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_audio_path_accepts_the_reported_shape() {
        let kb = Path::new(r"C:\kb");
        let ok = r"C:\kb\sessions\20260712T140335123Z-k4m2xp7q-sync.wav";
        assert_eq!(validate_audio_path(kb, ok).unwrap(), PathBuf::from(ok));
    }

    #[test]
    fn validate_audio_path_rejects_traversal_and_out_of_tree_paths() {
        let kb = Path::new(r"C:\kb");
        for raw in [
            // Not a .wav at all — the transcript is not revealable.
            r"C:\kb\sessions\s.jsonl",
            // Traversal segments. (An interior `.` is not listed: Rust path
            // components normalize it away, and the result genuinely resolves
            // inside the sessions directory, so accepting it is correct.)
            r"C:\kb\sessions\..\secrets.wav",
            r"..\sessions\s.wav",
            // Outside the sessions directory.
            r"C:\kb\other\s.wav",
            r"C:\kb\sessions\nested\s.wav",
            r"C:\elsewhere\sessions\s.wav",
        ] {
            assert!(
                validate_audio_path(kb, raw).is_err(),
                "should reject: {raw}"
            );
        }
    }
}
