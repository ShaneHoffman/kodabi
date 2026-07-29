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
//!
//! The same derivation now decides which *chats* are undistilled
//! ([`crate::chats`]), by subtracting the same `source:` set. The one deliberate
//! difference: an undistilled chat is never surfaced to the user. `chats/` is a
//! sibling of `sessions/` precisely so a chat transcript cannot read as an
//! unclaimed capture, so it earns no needs-attention row and no retry button —
//! the startup sweep just tries again, and the transcript is never pruned.
//!
//! The one persisted bit is the **dismissed marker**: a `.dismissed` sibling
//! of the `.jsonl` ([`naming::dismissed_sibling`]) recording that the user
//! waved the session off. That is user *intent*, which no walk of the vault
//! could re-derive — not a failure record: membership stays derived, and the
//! marker is only read for sessions the derivation already surfaced, so it can
//! never add a row or contradict what the markdown says. It self-heals with
//! the rest: a successful distill clears it ([`restore_session`]), the
//! retention sweep expires it alongside its session, and a marker orphaned by
//! a hand-deleted `.jsonl` is invisible to every surface (and expires with the
//! KeepDays sweep; under KeepAll it lingers harmlessly).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};

use crate::naming;
use crate::note::{NoteError, Source};
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
    /// Whether the user has waved this session off (its
    /// [`naming::dismissed_sibling`] marker exists). Membership is still
    /// derived; only the wave-off is stored — see the module docs.
    pub dismissed: bool,
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
    /// A `source:` value or session path that does not name a session
    /// artifact — a capture keyword (`manual`, `quick-capture`, …), a path
    /// outside the `sessions/<file>.jsonl` form, or a non-`.jsonl` path handed
    /// to the dismiss/restore/delete entry points. Rejected before any disk
    /// access.
    #[error("not a session source: {0}")]
    InvalidSource(String),
    /// A session transcript that exists but cannot be read or parsed.
    #[error(transparent)]
    RawSession(#[from] RawSessionError),
    /// A pagination token [`read_transcript_page`] did not mint. A caller
    /// error, not a storage fault.
    #[error("malformed pagination cursor {0:?}")]
    Cursor(String),
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
            dismissed: naming::dismissed_sibling(&path).is_file(),
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

/// Marks a needs-attention session as dismissed by writing its
/// [`naming::dismissed_sibling`] marker. Idempotent: dismissing an
/// already-dismissed session rewrites the marker, and a session whose
/// `.jsonl` is already gone (a retention sweep racing the click) is a no-op
/// rather than an orphaned marker.
pub fn dismiss_session(session_path: &Path) -> Result<(), SessionsError> {
    ensure_jsonl_session(session_path)?;
    if !session_path.is_file() {
        return Ok(());
    }
    let marker = naming::dismissed_sibling(session_path);
    // The dismissal instant, for a human inspecting the vault. Presence is
    // the whole signal — nothing parses this back, so a torn write still
    // reads as dismissed and plain `fs::write` suffices.
    let dismissed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    fs::write(&marker, dismissed_at).map_err(|source| SessionsError::Io {
        path: marker,
        source,
    })
}

/// Clears a session's dismissed marker so it counts as needing attention
/// again. Also the marker's exit on a successful distill: a session that
/// became a note must never keep one. `NotFound` is success — restoring an
/// undismissed session has nothing to undo.
pub fn restore_session(session_path: &Path) -> Result<(), SessionsError> {
    ensure_jsonl_session(session_path)?;
    remove_if_present(&naming::dismissed_sibling(session_path))
}

/// Permanently deletes a session and everything derived from it: the retained
/// recording, the dismissed marker, then the `.jsonl` transcript. The order is
/// deliberate — a removal failing partway leaves the transcript on disk, so
/// the session stays listed and the delete can be retried, rather than
/// stranding an invisible orphaned recording. Every `NotFound` is tolerated,
/// so the call is idempotent.
pub fn delete_session(session_path: &Path) -> Result<(), SessionsError> {
    ensure_jsonl_session(session_path)?;
    remove_if_present(&naming::audio_sibling(session_path))?;
    remove_if_present(&naming::dismissed_sibling(session_path))?;
    remove_if_present(session_path)
}

/// Deletes the session artifacts a note's `source:` points at, if any — so
/// deleting a distilled note leaves no orphaned recording or transcript (which
/// would otherwise resurface as a session needing attention, per this module's
/// docs).
///
/// A distilled note carries `source: sessions/<file>.jsonl`; this resolves that
/// [`Source::RawArtifact`] through the same traversal-safe
/// [`session_source_file_name`] gate [`read_session_artifacts`] uses, then calls
/// [`delete_session`], returning `Ok(true)` when a session was targeted. A
/// capture keyword (`manual`, `quick-capture`, …) or any raw-artifact value that
/// does not name a `sessions/<file>.jsonl` artifact resolves to nothing and
/// returns `Ok(false)` without touching disk. The underlying delete is
/// idempotent, so a session whose transcript was already retention-pruned is a
/// clean no-op (any surviving recording is still removed).
pub fn delete_session_for_source(
    vault_root: &Path,
    source: &Source,
) -> Result<bool, SessionsError> {
    let Source::RawArtifact(raw) = source else {
        return Ok(false);
    };
    let Some(file_name) = session_source_file_name(raw) else {
        return Ok(false);
    };
    let path = vault_root.join(SESSIONS_DIR).join(file_name);
    delete_session(&path)?;
    Ok(true)
}

/// Refuses a path that is not a `.jsonl` session transcript, so a marker or
/// delete can never target an arbitrary file. The Tauri layer already confines
/// the path to `<vault>/sessions/`; this is core's own safety belt, mirroring
/// the `.jsonl` gate in [`retention::apply_post_distill`].
fn ensure_jsonl_session(session_path: &Path) -> Result<(), SessionsError> {
    if session_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
        Ok(())
    } else {
        Err(SessionsError::InvalidSource(
            session_path.display().to_string(),
        ))
    }
}

/// Removes a file, treating an already-gone target as success.
fn remove_if_present(path: &Path) -> Result<(), SessionsError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(SessionsError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
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

/// Smallest and largest `limit` [`read_transcript_page`] honors, mirroring the
/// `get_meeting_transcript` `inputSchema` bounds.
pub const MIN_TRANSCRIPT_LIMIT: u32 = 1;
pub const MAX_TRANSCRIPT_LIMIT: u32 = 1000;

/// Cursor-based pagination envelope for [`read_transcript_page`], mirroring the
/// `PageInfo` `$def` of `docs/MCP_TOOL_SURFACE.md`. A local mirror rather than a
/// shared type, following `vault::ProjectPageInfo` — each paginated surface owns
/// its own cursor codec. `total_estimate` is always exact here: the whole
/// transcript is read into memory, so its size *is* the total.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TranscriptPageInfo {
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub total_estimate: Option<u64>,
}

/// One page of a meeting's transcript — the `get_meeting_transcript` payload
/// minus the note metadata the MCP layer adds.
///
/// `transcript_available` is `false` (with empty `segments`) both when retention
/// pruned the `.jsonl` and when the note's `source` never named one; see
/// [`read_transcript_page`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TranscriptPage {
    pub transcript_available: bool,
    pub segments: Vec<TranscriptSegment>,
    pub page: TranscriptPageInfo,
}

/// Reads one page of a note's transcript, ordered by `start_ms` ascending.
///
/// Wraps [`read_session_artifacts`] with the paging the
/// `get_meeting_transcript` tool needs. Two "no transcript" cases collapse to
/// the same honest answer — `transcript_available: false` with no segments:
/// retention pruned the `.jsonl`, and the note's `source` is a capture keyword
/// (`manual`, `quick-capture`, …) that never named a session at all. Neither is
/// an error: the note is still a meeting, it simply has nothing stored to page.
/// A transcript that exists but cannot be parsed stays a real error.
///
/// `limit` is clamped to `MIN_TRANSCRIPT_LIMIT..=MAX_TRANSCRIPT_LIMIT`. The
/// cursor is validated before any disk access, so a malformed token fails the
/// same way whatever the vault holds.
pub fn read_transcript_page(
    vault_root: &Path,
    source: &str,
    limit: u32,
    cursor: Option<&str>,
) -> Result<TranscriptPage, SessionsError> {
    let limit = limit.clamp(MIN_TRANSCRIPT_LIMIT, MAX_TRANSCRIPT_LIMIT) as usize;
    let cursor = cursor.map(decode_transcript_cursor).transpose()?;

    let artifacts = match read_session_artifacts(vault_root, source) {
        Ok(artifacts) => artifacts,
        Err(SessionsError::InvalidSource(_)) => SessionArtifacts {
            transcript_available: false,
            segments: Vec::new(),
            audio_path: None,
        },
        Err(err) => return Err(err),
    };

    let total = artifacts.segments.len();
    // Keyset on the segment's own `index`, which `raw_session::assemble` assigns
    // after sorting by `start_ms` — so it is the transcript's total order, and
    // resuming past the boundary segment can neither skip nor repeat a row.
    let start = match cursor {
        Some(last_index) => artifacts
            .segments
            .iter()
            .position(|segment| segment.index > last_index)
            .unwrap_or(total),
        None => 0,
    };
    let end = start.saturating_add(limit).min(total);
    let segments = artifacts.segments[start..end].to_vec();

    let has_more = end < total;
    let next_cursor = segments
        .last()
        .filter(|_| has_more)
        .map(|segment| encode_transcript_cursor(segment.index));

    Ok(TranscriptPage {
        transcript_available: artifacts.transcript_available,
        segments,
        page: TranscriptPageInfo {
            has_more,
            next_cursor,
            total_estimate: Some(total as u64),
        },
    })
}

/// Encodes a transcript cursor: the boundary segment's `index`.
fn encode_transcript_cursor(index: u64) -> String {
    format!("v1:{index}")
}

/// Decodes a transcript cursor, rejecting anything [`encode_transcript_cursor`]
/// did not produce.
fn decode_transcript_cursor(raw: &str) -> Result<u64, SessionsError> {
    raw.strip_prefix("v1:")
        .and_then(|index| index.parse::<u64>().ok())
        .ok_or_else(|| SessionsError::Cursor(raw.to_string()))
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
    fn dismissed_session_is_listed_with_its_marker_flag() {
        let vault = tempdir().unwrap();
        let waved = write_session(vault.path(), instant(9), Some("waved"), "spoken");
        write_session(vault.path(), instant(14), Some("active"), "spoken");
        dismiss_session(&waved).unwrap();

        let failed = list_failed_sessions(vault.path()).unwrap();

        assert_eq!(
            failed.len(),
            2,
            "dismissal must not drop a session from the listing"
        );
        assert_eq!(failed[0].slug.as_deref(), Some("active"));
        assert!(!failed[0].dismissed);
        assert_eq!(failed[1].slug.as_deref(), Some("waved"));
        assert!(failed[1].dismissed);
    }

    #[test]
    fn dismiss_and_restore_round_trip_the_marker() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("waved"), "spoken");
        let marker = naming::dismissed_sibling(&session);

        dismiss_session(&session).unwrap();
        assert!(marker.is_file());
        // Dismissing again is a rewrite, not an error.
        dismiss_session(&session).unwrap();

        restore_session(&session).unwrap();
        assert!(!marker.exists());
        // Restoring an undismissed session has nothing to undo.
        restore_session(&session).unwrap();
    }

    #[test]
    fn dismissing_a_missing_session_creates_no_orphan_marker() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("gone"), "spoken");
        fs::remove_file(&session).unwrap();

        dismiss_session(&session).unwrap();

        assert!(!naming::dismissed_sibling(&session).exists());
    }

    #[test]
    fn dismiss_restore_and_delete_refuse_a_non_jsonl_path() {
        let vault = tempdir().unwrap();
        let sessions = vault.path().join(SESSIONS_DIR);
        fs::create_dir_all(&sessions).unwrap();
        let recording = sessions.join("keep.wav");
        fs::write(&recording, "").unwrap();

        for result in [
            dismiss_session(&recording),
            restore_session(&recording),
            delete_session(&recording),
        ] {
            assert!(matches!(result, Err(SessionsError::InvalidSource(_))));
        }
        assert!(recording.is_file(), "a refused delete must not touch disk");
        assert_eq!(
            fs::read_dir(&sessions).unwrap().count(),
            1,
            "a refused dismiss must not write a marker"
        );
    }

    #[test]
    fn a_dismissed_marker_never_lists_as_a_session_itself() {
        // An orphaned marker (its `.jsonl` hand-deleted) is inert: the
        // jsonl-only filter keeps it out of the needs-attention list.
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("orphan"), "spoken");
        dismiss_session(&session).unwrap();
        fs::remove_file(&session).unwrap();

        assert_eq!(list_failed_sessions(vault.path()).unwrap(), Vec::new());
    }

    #[test]
    fn delete_session_removes_transcript_recording_and_marker() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("doomed"), "spoken");
        let recording = naming::audio_sibling(&session);
        fs::write(&recording, "").unwrap();
        dismiss_session(&session).unwrap();

        delete_session(&session).unwrap();

        assert!(!session.exists());
        assert!(!recording.exists());
        assert!(!naming::dismissed_sibling(&session).exists());
        assert_eq!(list_failed_sessions(vault.path()).unwrap(), Vec::new());
    }

    #[test]
    fn delete_session_is_idempotent_when_already_gone() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("gone"), "spoken");

        delete_session(&session).unwrap();
        // A second delete finds nothing left and still succeeds.
        delete_session(&session).unwrap();
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

    #[test]
    fn delete_session_for_source_removes_a_notes_paired_artifacts() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("paired"), "spoken");
        let recording = naming::audio_sibling(&session);
        fs::write(&recording, "").unwrap();
        dismiss_session(&session).unwrap();
        let source = Source::parse(&source_value(
            session.file_name().unwrap().to_str().unwrap(),
        ))
        .unwrap();

        let removed = delete_session_for_source(vault.path(), &source).unwrap();

        assert!(removed, "a resolvable session source reports a removal");
        assert!(!session.exists());
        assert!(!recording.exists());
        assert!(!naming::dismissed_sibling(&session).exists());
        assert_eq!(list_failed_sessions(vault.path()).unwrap(), Vec::new());
    }

    #[test]
    fn delete_session_for_source_ignores_a_keyword_source() {
        let vault = tempdir().unwrap();
        // An unrelated session on disk must survive a keyword-source cleanup.
        let bystander = write_session(vault.path(), instant(14), Some("kept"), "spoken");

        let removed =
            delete_session_for_source(vault.path(), &Source::parse("manual").unwrap()).unwrap();

        assert!(!removed, "a capture keyword names no artifact");
        assert!(bystander.is_file());
    }

    #[test]
    fn delete_session_for_source_ignores_a_non_session_raw_artifact() {
        let vault = tempdir().unwrap();
        let bystander = write_session(vault.path(), instant(14), Some("kept"), "spoken");

        // A raw artifact that parses but does not name a `sessions/<file>.jsonl`
        // resolves to nothing and touches no disk. (Traversal never reaches here
        // at all: `Source::parse` rejects `.`/`..` segments up front.)
        for source in ["sessions/recording.wav", "attachments/photo.png"] {
            let removed =
                delete_session_for_source(vault.path(), &Source::parse(source).unwrap()).unwrap();
            assert!(!removed, "should not resolve: {source}");
        }
        assert!(bystander.is_file());
    }

    #[test]
    fn delete_session_for_source_is_idempotent_when_already_pruned() {
        let vault = tempdir().unwrap();
        let session = write_session(vault.path(), instant(14), Some("gone"), "spoken");
        let source = Source::parse(&source_value(
            session.file_name().unwrap().to_str().unwrap(),
        ))
        .unwrap();
        fs::remove_file(&session).unwrap();

        // The session it named is already gone; resolving it still succeeds and
        // reports the target as handled.
        let removed = delete_session_for_source(vault.path(), &source).unwrap();
        assert!(removed);
    }

    // --- read_transcript_page ------------------------------------------------

    /// Writes a session of `count` alternating-channel segments 1s apart, and
    /// returns its `source:` value.
    fn write_multi_segment_session(vault: &Path, count: u64) -> String {
        let segments: Vec<TranscriptSegment> = (0..count)
            .map(|index| TranscriptSegment {
                index,
                channel: if index % 2 == 0 {
                    Channel::You
                } else {
                    Channel::Them
                },
                speaker: None,
                start_ms: index * 1_000,
                end_ms: index * 1_000 + 500,
                text: format!("segment {index}"),
            })
            .collect();
        let path = raw_session::write_raw_session(
            &vault.join(SESSIONS_DIR),
            instant(14),
            &device(),
            Some("long talk"),
            &segments,
        )
        .unwrap();
        source_value(path.file_name().unwrap().to_str().unwrap())
    }

    #[test]
    fn transcript_page_walks_every_segment_exactly_once() {
        let vault = tempdir().unwrap();
        let source = write_multi_segment_session(vault.path(), 7);

        let mut seen = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = read_transcript_page(vault.path(), &source, 3, cursor.as_deref()).unwrap();
            assert!(page.transcript_available);
            // The total is exact on every page, not just the first.
            assert_eq!(page.page.total_estimate, Some(7));
            seen.extend(page.segments.iter().map(|segment| segment.index));
            match page.page.next_cursor {
                Some(next) => cursor = Some(next),
                None => {
                    assert!(!page.page.has_more);
                    break;
                }
            }
        }

        assert_eq!(seen, (0..7).collect::<Vec<u64>>());
    }

    #[test]
    fn transcript_page_reports_ordering_channels_and_offsets() {
        let vault = tempdir().unwrap();
        let source = write_multi_segment_session(vault.path(), 3);

        let page = read_transcript_page(vault.path(), &source, 200, None).unwrap();

        assert_eq!(page.segments.len(), 3);
        assert!(!page.page.has_more);
        assert!(page.page.next_cursor.is_none());
        // Ordered by start_ms ascending, with per-channel attribution intact.
        let offsets: Vec<u64> = page.segments.iter().map(|s| s.start_ms).collect();
        assert_eq!(offsets, [0, 1_000, 2_000]);
        assert_eq!(page.segments[0].channel, Channel::You);
        assert_eq!(page.segments[1].channel, Channel::Them);
        assert_eq!(page.segments[0].speaker, None);
    }

    #[test]
    fn transcript_page_clamps_limit_to_the_schema_bounds() {
        let vault = tempdir().unwrap();
        let source = write_multi_segment_session(vault.path(), 5);

        // 0 clamps up to 1 rather than returning an empty page forever.
        let page = read_transcript_page(vault.path(), &source, 0, None).unwrap();
        assert_eq!(page.segments.len(), 1);
        assert!(page.page.has_more);

        // An over-large limit clamps down but still serves everything here.
        let page = read_transcript_page(vault.path(), &source, u32::MAX, None).unwrap();
        assert_eq!(page.segments.len(), 5);
        assert!(!page.page.has_more);
    }

    #[test]
    fn pruned_transcript_is_unavailable_not_an_error() {
        let vault = tempdir().unwrap();
        let source = write_multi_segment_session(vault.path(), 3);
        let file_name = source.strip_prefix("sessions/").unwrap();
        fs::remove_file(vault.path().join(SESSIONS_DIR).join(file_name)).unwrap();

        let page = read_transcript_page(vault.path(), &source, 200, None).unwrap();

        assert!(!page.transcript_available);
        assert!(page.segments.is_empty());
        assert!(!page.page.has_more);
        assert_eq!(page.page.total_estimate, Some(0));
    }

    #[test]
    fn a_capture_keyword_source_is_unavailable_not_an_error() {
        let vault = tempdir().unwrap();

        // A meeting note captured without a session artifact (`manual`,
        // `quick-capture`, …) has nothing to page — the same honest answer as a
        // pruned transcript, not an `InvalidSource` error.
        for source in ["manual", "quick-capture", "transcript"] {
            let page = read_transcript_page(vault.path(), source, 200, None).unwrap();
            assert!(!page.transcript_available, "{source}");
            assert!(page.segments.is_empty(), "{source}");
        }
    }

    #[test]
    fn a_tampered_transcript_cursor_is_rejected() {
        let vault = tempdir().unwrap();
        let source = write_multi_segment_session(vault.path(), 3);

        for bad in ["", "v1:", "v2:0", "0", "v1:abc", "v1:-1"] {
            assert!(
                matches!(
                    read_transcript_page(vault.path(), &source, 2, Some(bad)),
                    Err(SessionsError::Cursor(_))
                ),
                "cursor {bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn a_cursor_past_the_last_segment_yields_an_empty_final_page() {
        let vault = tempdir().unwrap();
        let source = write_multi_segment_session(vault.path(), 3);

        // A cursor minted before the transcript shrank (retention rewrote it, a
        // hand edit trimmed it) must not panic or wrap around.
        let page = read_transcript_page(vault.path(), &source, 2, Some("v1:99")).unwrap();

        assert!(page.segments.is_empty());
        assert!(!page.page.has_more);
        assert!(page.page.next_cursor.is_none());
    }
}
