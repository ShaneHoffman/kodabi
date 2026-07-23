//! Reading the `sessions/` store from a note's point of view: the "needs
//! attention" list of captured sessions that never became a note, and
//! [`read_session_artifacts`] — resolving a distilled note's `source:` back to
//! its raw transcript and retained recording.
//!
//! A distill failure leaves the raw `.jsonl` untouched on disk so the run can
//! be retried (see [`crate::distill`]'s module docs), but nothing on disk marks
//! it as failed. Rather than persist a failure record — which would be a second
//! source of truth to keep honest, against the markdown-is-truth rule
//! (FOUNDING_DOC §3.6) — membership is **derived**:
//!
//! ```text
//! needs attention = <vault>/sessions/*.jsonl
//!                 − sessions some note already claims via its `source:` field
//!                 − silent captures (nothing was ever said)
//! ```
//!
//! Deriving it this way is what makes the surface survive a restart, and it
//! covers the case no event could: the app dying mid-distill, where no `Error`
//! was ever emitted. It also self-heals — a session deleted by the retention
//! sweep simply stops being listed.
//!
//! Two consequences worth knowing:
//!
//! - Deleting a note whose session still exists makes that session reappear as
//!   needing attention. That is the honest reading of "a session with no note",
//!   not a bug.
//! - A session claimed only by a note whose frontmatter no longer parses, or
//!   whose folder can't be read at all, reads as unreferenced (both are skipped
//!   by the walk), so it can resurface. That direction is deliberate: the walk
//!   stays tolerant so one bad file or ACL-denied folder can't blank the whole
//!   list, and the cost of erring this way is a duplicate note if the user
//!   retries, against a silently dropped meeting if it erred the other way.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};

use crate::naming;
use crate::note::NoteError;
use crate::raw_session::{self, RawSessionError, TranscriptSegment};
use crate::retention;
use crate::vault;

/// KB-root directory holding raw session transcripts, as
/// [`crate::raw_session::write_raw_session`]'s callers compose it and
/// `note::RESERVED_ROOT_DIRS` reserves it.
const SESSIONS_DIR: &str = "sessions";

/// A captured session no note claims: its distill failed, or never ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedSession {
    /// Absolute path to the `.jsonl` — the exact value a retry hands back to
    /// the distill entry point.
    pub path: PathBuf,
    pub file_name: String,
    /// The slug segment of the filename when the name follows the session
    /// scheme, as a display seed. `None` for a hand-imported name.
    pub slug: Option<String>,
    /// Capture instant as RFC 3339 UTC (`Z`): the filename timestamp, else the
    /// file's mtime, else now.
    pub captured_at: String,
}

/// Why deriving the needs-attention list failed outright. Per-file and
/// per-folder trouble never lands here — an unreadable session is *listed* (it
/// needs attention), and an unparseable note or unreadable project subtree is
/// skipped by [`vault::collect_raw_artifact_sources`] rather than failing the
/// walk — so this is only a sessions directory or vault root that can't be
/// enumerated at all. One ACL-denied folder must never blank the whole list:
/// the surface exists precisely so a failed capture is not silently dropped.
#[derive(Debug, thiserror::Error)]
pub enum SessionsError {
    #[error("failed to read sessions directory {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Notes(#[from] NoteError),
    /// A `source:` value that does not name a session artifact — a capture
    /// keyword (`manual`, `quick-capture`, …) or a path outside the
    /// `sessions/<file>.jsonl` form. Rejected before any disk access.
    #[error("not a session source: {0}")]
    InvalidSource(String),
    /// A session transcript that exists but cannot be read or parsed.
    #[error(transparent)]
    RawSession(#[from] RawSessionError),
}

/// Lists the sessions needing attention, newest capture first.
///
/// Cost is linear in the size of the vault: one walk that reads and parses
/// every note to collect the claimed-session set (the same class of walk as
/// [`crate::vault::find_note_anywhere`]), plus a streamed prefix of each
/// unclaimed session to tell a silent capture from a real one. Fine for a
/// personal vault, and it only runs when a list that shows these is on screen.
pub fn list_failed_sessions(vault_root: &Path) -> Result<Vec<FailedSession>, SessionsError> {
    let claimed = vault::collect_raw_artifact_sources(vault_root)?;
    let sessions_dir = vault_root.join(SESSIONS_DIR);

    let entries = match fs::read_dir(&sessions_dir) {
        Ok(entries) => entries,
        // Nothing captured yet is an empty list, not an error.
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(SessionsError::Io {
                path: sessions_dir,
                source,
            })
        }
    };

    let mut failed = Vec::new();
    for entry in entries {
        // One unreadable directory entry must not blank the whole list.
        let Ok(entry) = entry else { continue };
        // Regular top-level `.jsonl` files only — the same conservative filter
        // `retention::prune_sessions` applies, so the in-flight spill directory
        // and the `.tmp` scratch files of a write in progress are both skipped.
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if claimed.contains(&source_value(&file_name)) {
            continue;
        }
        if !needs_attention(&path) {
            continue;
        }

        let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
        let captured_at = retention::session_time(&file_name, mtime).unwrap_or_else(Utc::now);
        failed.push(FailedSession {
            slug: naming::parse_session_filename(&file_name).and_then(|parsed| parsed.slug),
            captured_at: captured_at.to_rfc3339_opts(SecondsFormat::Secs, true),
            file_name,
            path,
        });
    }

    // Newest first, tie-broken by name so the order is total and stable.
    failed.sort_by(|a, b| {
        b.captured_at
            .cmp(&a.captured_at)
            .then_with(|| a.file_name.cmp(&b.file_name))
    });
    Ok(failed)
}

/// Whether an unclaimed session is worth surfacing: a real transcript that
/// failed to distill, rather than a silent capture nobody needs to act on.
///
/// A session that can't be read or parsed **does** need attention: distilling
/// it would fail the same way, so the user should see it rather than have it
/// silently withheld. The one exception is a file that vanished mid-scan (a
/// retention sweep racing this walk), which is simply gone, not broken.
fn needs_attention(path: &Path) -> bool {
    match raw_session::is_silent_on_disk(path) {
        Ok(silent) => !silent,
        Err(RawSessionError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            false
        }
        Err(_) => true,
    }
}

/// The `source:` value a note carries for the session named `file_name` —
/// vault-relative with forward slashes, exactly the form
/// [`crate::distill::distill_session`] writes.
fn source_value(file_name: &str) -> String {
    format!("{SESSIONS_DIR}/{file_name}")
}

/// What a distilled note's `source:` field pairs it with: the raw transcript
/// (when the `.jsonl` still exists) and the retained recording (when its
/// `.wav` sibling does). The two are independent — retention can prune the
/// transcript while a recording survives a failed sibling delete, and a
/// recording whose write failed leaves the transcript alone — so presence is
/// reported separately and every combination is representable.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionArtifacts {
    /// Whether the raw transcript is still on disk. `false` is the
    /// retention-pruned state, not an error.
    pub transcript_available: bool,
    /// The transcript's segments, in order; empty when unavailable.
    pub segments: Vec<TranscriptSegment>,
    /// Absolute path to the retained recording, when it exists.
    pub audio_path: Option<PathBuf>,
}

/// Resolves a note's `source:` value to its session artifacts.
///
/// `source` must be the raw-artifact form [`crate::distill::distill_session`]
/// writes — `sessions/<file>.jsonl` — anything else (a capture keyword, a
/// traversal attempt) is [`SessionsError::InvalidSource`]. A missing
/// transcript yields `transcript_available: false` with empty segments; a
/// transcript that exists but can't be parsed is a real error, matching what
/// a distill retry would hit.
pub fn read_session_artifacts(
    vault_root: &Path,
    source: &str,
) -> Result<SessionArtifacts, SessionsError> {
    let file_name = session_source_file_name(source)
        .ok_or_else(|| SessionsError::InvalidSource(source.to_string()))?;
    let path = vault_root.join(SESSIONS_DIR).join(file_name);

    let (transcript_available, segments) = match raw_session::read_raw_session(&path) {
        Ok(segments) => (true, segments),
        Err(RawSessionError::Io { ref source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            (false, Vec::new())
        }
        Err(err) => return Err(err.into()),
    };

    let audio = naming::audio_sibling(&path);
    let audio_path = audio.is_file().then_some(audio);

    Ok(SessionArtifacts {
        transcript_available,
        segments,
        audio_path,
    })
}

/// Lexically validates a `source:` value as a session reference and returns
/// the bare file name. The joined `<vault>/sessions/<file_name>` must stay
/// inside the sessions directory, so the name must be a single plain path
/// component: no separators, no drive/ADS colon, no dot segments. Only the
/// `.jsonl` form distill writes qualifies.
fn session_source_file_name(source: &str) -> Option<&str> {
    let file_name = source.strip_prefix(SESSIONS_DIR)?.strip_prefix('/')?;
    if file_name.is_empty()
        || file_name.contains(['/', '\\', ':'])
        || file_name == "."
        || file_name == ".."
    {
        return None;
    }
    file_name.ends_with(".jsonl").then_some(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;
    use crate::note::{Note, NoteId, NoteType, Routing, Source, INBOX};
    use crate::raw_session::TranscriptSegment;
    use crate::transcription::Channel;
    use chrono::{DateTime, TimeZone, Utc};
    use tempfile::tempdir;

    fn device() -> DeviceId {
        DeviceId::parse("k4m2xp7q").unwrap()
    }

    fn instant(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, hour, 3, 35).unwrap()
    }

    fn segment(text: &str) -> TranscriptSegment {
        TranscriptSegment {
            index: 0,
            channel: Channel::You,
            speaker: None,
            start_ms: 0,
            end_ms: 500,
            text: text.to_owned(),
        }
    }

    /// Writes a session transcript into `<vault>/sessions/` and returns its path.
    fn write_session(vault: &Path, at: DateTime<Utc>, slug: Option<&str>, text: &str) -> PathBuf {
        raw_session::write_raw_session(
            &vault.join(SESSIONS_DIR),
            at,
            &device(),
            slug,
            &[segment(text)],
        )
        .unwrap()
    }

    /// Files a note in `project` claiming `session` as its source. The Inbox is
    /// always routing-scored (a `Manual` note may not live there), so the
    /// routing variant follows the target.
    fn write_note_for(vault: &Path, project: &str, session: &Path) {
        let file_name = session.file_name().unwrap().to_str().unwrap();
        let routing = if project == INBOX {
            Routing::Routed {
                project: project.to_string(),
                confidence: 0.4,
            }
        } else {
            Routing::Manual {
                project: project.to_string(),
            }
        };
        let note = Note::new(
            NoteId::generate().unwrap(),
            NoteType::Meeting,
            routing,
            "2026-07-12",
            Vec::new(),
            Source::parse(&source_value(file_name)).unwrap(),
            "Distilled body.",
        )
        .unwrap();
        crate::note::write_note(vault, &note, Some("distilled")).unwrap();
    }

    #[test]
    fn unreferenced_session_with_text_is_listed() {
        let vault = tempdir().unwrap();
        let session = write_session(
            vault.path(),
            instant(14),
            Some("budget sync"),
            "hello there",
        );

        let failed = list_failed_sessions(vault.path()).unwrap();

        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].path, session);
        assert_eq!(
            failed[0].file_name,
            session.file_name().unwrap().to_str().unwrap()
        );
        assert_eq!(failed[0].slug.as_deref(), Some("budget-sync"));
        assert_eq!(failed[0].captured_at, "2026-07-12T14:03:35Z");
    }

    #[test]
    fn note_referenced_sessions_are_excluded() {
        let vault = tempdir().unwrap();
        let inboxed = write_session(vault.path(), instant(9), Some("one"), "spoken");
        let filed = write_session(vault.path(), instant(10), Some("two"), "spoken");
        let orphan = write_session(vault.path(), instant(11), Some("three"), "spoken");

        write_note_for(vault.path(), INBOX, &inboxed);
        write_note_for(vault.path(), "Growth/Q3", &filed);

        let failed = list_failed_sessions(vault.path()).unwrap();

        assert_eq!(failed.len(), 1, "only the unclaimed session should list");
        assert_eq!(failed[0].path, orphan);
    }

    #[test]
    fn silent_and_empty_sessions_are_excluded() {
        let vault = tempdir().unwrap();
        write_session(vault.path(), instant(9), Some("whitespace"), "   \t  ");
        // A capture with no segments at all — distill calls this benign too.
        raw_session::write_raw_session(
            &vault.path().join(SESSIONS_DIR),
            instant(10),
            &device(),
            Some("empty"),
            &[],
        )
        .unwrap();

        assert_eq!(list_failed_sessions(vault.path()).unwrap(), Vec::new());
    }

    #[test]
    fn tmp_files_subdirs_and_inflight_are_ignored() {
        let vault = tempdir().unwrap();
        let sessions = vault.path().join(SESSIONS_DIR);
        let inflight = sessions.join("inflight").join("20260712-140335-k4m2xp7q");
        fs::create_dir_all(&inflight).unwrap();
        // An in-flight spill that happens to hold a `.jsonl`, a staging scratch
        // file, and a non-session file: none of them are top-level sessions.
        fs::write(inflight.join("spill.jsonl"), "{}").unwrap();
        fs::write(sessions.join(".raw-session.1234.0.tmp"), "{}").unwrap();
        fs::write(sessions.join("notes.txt"), "not a session").unwrap();

        assert_eq!(list_failed_sessions(vault.path()).unwrap(), Vec::new());
    }

    #[test]
    fn missing_sessions_dir_yields_empty() {
        let vault = tempdir().unwrap();

        assert_eq!(list_failed_sessions(vault.path()).unwrap(), Vec::new());
    }

    #[test]
    fn unparseable_session_is_listed_with_mtime_fallback() {
        let vault = tempdir().unwrap();
        let sessions = vault.path().join(SESSIONS_DIR);
        fs::create_dir_all(&sessions).unwrap();
        // Garbage content under a name that doesn't follow the session scheme:
        // it would fail to distill, so it needs attention, and its timestamp
        // falls back to the file's mtime.
        let path = sessions.join("hand-imported.jsonl");
        fs::write(&path, "not json at all").unwrap();

        let failed = list_failed_sessions(vault.path()).unwrap();

        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].path, path);
        assert_eq!(failed[0].slug, None);
        assert!(
            failed[0].captured_at.ends_with('Z'),
            "captured_at must be UTC RFC 3339: {}",
            failed[0].captured_at
        );
        assert!(DateTime::parse_from_rfc3339(&failed[0].captured_at).is_ok());
    }

    #[test]
    fn retained_recording_never_lists_as_a_failed_session() {
        // A `.wav` sibling is a recording, not a session: the jsonl-only
        // filter must keep it out of the needs-attention list even when its
        // transcript has been pruned out from under it.
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("kept"), "spoken");
        fs::write(naming::audio_sibling(&session), "").unwrap();
        fs::remove_file(&session).unwrap();

        assert_eq!(list_failed_sessions(vault.path()).unwrap(), Vec::new());
    }

    #[test]
    fn artifacts_resolve_transcript_and_recording() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("sync"), "hello there");
        let recording = naming::audio_sibling(&session);
        fs::write(&recording, "").unwrap();
        let source = source_value(session.file_name().unwrap().to_str().unwrap());

        let artifacts = read_session_artifacts(vault.path(), &source).unwrap();

        assert!(artifacts.transcript_available);
        assert_eq!(artifacts.segments.len(), 1);
        assert_eq!(artifacts.segments[0].text, "hello there");
        assert_eq!(artifacts.audio_path, Some(recording));
    }

    #[test]
    fn artifacts_report_a_pruned_transcript_without_error() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("gone"), "spoken");
        // Retention pruned the transcript but a recording survives — the note
        // still resolves what remains.
        let recording = naming::audio_sibling(&session);
        fs::write(&recording, "").unwrap();
        let source = source_value(session.file_name().unwrap().to_str().unwrap());
        fs::remove_file(&session).unwrap();

        let artifacts = read_session_artifacts(vault.path(), &source).unwrap();

        assert!(!artifacts.transcript_available);
        assert!(artifacts.segments.is_empty());
        assert_eq!(artifacts.audio_path, Some(recording));
    }

    #[test]
    fn artifacts_omit_a_missing_recording() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("no-audio"), "spoken");
        let source = source_value(session.file_name().unwrap().to_str().unwrap());

        let artifacts = read_session_artifacts(vault.path(), &source).unwrap();

        assert!(artifacts.transcript_available);
        assert_eq!(artifacts.audio_path, None);
    }

    #[test]
    fn artifacts_reject_non_session_sources() {
        let vault = tempdir().unwrap();
        for source in [
            // Capture keywords are valid `source:` values but name no artifact.
            "manual",
            "quick-capture",
            "transcript",
            // Traversal and separator injection must never reach the disk.
            "sessions/../secrets.jsonl",
            "sessions/nested/deep.jsonl",
            "sessions/nested\\deep.jsonl",
            "sessions/C:evil.jsonl",
            "sessions/",
            // Only the `.jsonl` raw-artifact form resolves.
            "sessions/recording.wav",
            "raw/20260712T140335123Z-k4m2xp7q.jsonl",
        ] {
            assert!(
                matches!(
                    read_session_artifacts(vault.path(), source),
                    Err(SessionsError::InvalidSource(_))
                ),
                "should reject: {source}"
            );
        }
    }

    #[test]
    fn sessions_sort_newest_first() {
        let vault = tempdir().unwrap();
        write_session(vault.path(), instant(9), Some("earliest"), "spoken");
        write_session(vault.path(), instant(16), Some("latest"), "spoken");
        write_session(vault.path(), instant(12), Some("middle"), "spoken");

        let failed = list_failed_sessions(vault.path()).unwrap();

        let slugs: Vec<&str> = failed.iter().map(|s| s.slug.as_deref().unwrap()).collect();
        assert_eq!(slugs, ["latest", "middle", "earliest"]);
    }
}
