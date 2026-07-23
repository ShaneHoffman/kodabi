//! Pruning raw session artifacts per the user's [`RetentionPolicy`].
//!
//! Sessions are the `.jsonl` transcripts [`crate::raw_session::write_raw_session`]
//! drops in the KB's `sessions/` directory, plus their retained `.wav`
//! recordings and `.dismissed` needs-attention markers (same filename stem —
//! see [`crate::naming::audio_sibling`] and
//! [`crate::naming::dismissed_sibling`]). This module enforces the retention
//! policy over them two ways:
//!
//! - [`prune_sessions`] — a sweep, for [`RetentionPolicy::KeepDays`]: delete
//!   session artifacts older than the cutoff. Run on a schedule (startup +
//!   periodically) and whenever the policy changes.
//! - [`apply_post_distill`] — forward-only enforcement of
//!   [`RetentionPolicy::DiscardAfterDistill`]: delete a single session (its
//!   recording and marker included) the moment it has been successfully
//!   distilled into a note.
//!
//! Deletion is deliberately conservative: only regular `.jsonl`, `.wav`, and
//! `.dismissed` files directly in the sessions directory are ever touched
//! (never the `.tmp` scratch files an in-flight write stages, subdirectories,
//! or anything else), and a session whose age can't be determined is kept
//! rather than guessed-at.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use crate::naming;
use crate::settings::RetentionPolicy;

/// What a [`prune_sessions`] sweep did. `failed` collects per-file deletion
/// errors so one unremovable file (a lock, a permission quirk) never aborts
/// the rest of the sweep.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub deleted: Vec<PathBuf>,
    pub failed: Vec<PathBuf>,
}

/// Deletes raw sessions older than the policy's cutoff. A no-op (returning an
/// empty report) for [`RetentionPolicy::KeepAll`] and
/// [`RetentionPolicy::DiscardAfterDistill`] — the latter is enforced at
/// distill time by [`apply_post_distill`], not by sweeping, so it never
/// retro-prunes sessions captured before the policy was chosen.
///
/// A missing sessions directory is not an error (nothing has been captured
/// yet) — it yields an empty report. `now` is injected so the age decision is
/// deterministic under test.
///
/// The sweep ages sessions purely by capture time and **never consults note
/// existence**: a session whose distill failed (or never ran) is pruned once it
/// crosses the cutoff, exactly like a distilled one. This is deliberate — a
/// KeepDays user asked for raw transcripts to expire on a schedule, and a
/// failed distill doesn't earn an indefinite exception to that. The
/// needs-attention surface (`crate::sessions::list_failed_sessions`) derives
/// from disk, so an expired failed session simply drops out of the list; the
/// shell announces the deletion via its `sessions:changed` event so any open
/// list refetches.
pub fn prune_sessions(
    sessions_dir: &Path,
    policy: RetentionPolicy,
    now: DateTime<Utc>,
) -> io::Result<PruneReport> {
    let days = match policy {
        RetentionPolicy::KeepDays { days } => days.get(),
        RetentionPolicy::KeepAll | RetentionPolicy::DiscardAfterDistill => {
            return Ok(PruneReport::default())
        }
    };
    let cutoff = now - chrono::Duration::days(i64::from(days));

    let entries = match fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(PruneReport::default()),
        Err(err) => return Err(err),
    };

    let mut report = PruneReport::default();
    for entry in entries {
        // A single unreadable directory entry shouldn't abort the sweep.
        let Ok(entry) = entry else { continue };
        // Regular files only — never recurse into subdirectories.
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        // Only `.jsonl` transcripts, their retained `.wav` recordings, and
        // their `.dismissed` needs-attention markers; this also excludes the
        // `.tmp` scratch files an in-flight write stages before its atomic
        // link/rename. All three artifacts share a filename stem, so they age
        // identically and expire as a trio — which is also what reaps a marker
        // orphaned by a hand-deleted transcript.
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("jsonl" | "wav" | "dismissed")
        ) {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
        if is_expired(file_name, mtime, cutoff) {
            match fs::remove_file(&path) {
                Ok(()) => report.deleted.push(path),
                // Already gone — a concurrent sweep (the periodic schedule
                // racing an on-change `spawn_prune`) or the post-distill
                // discard beat us to it. That's success, not failure; only a
                // real error counts as failed (mirrors `apply_post_distill`).
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(_) => report.failed.push(path),
            }
        }
    }
    Ok(report)
}

/// Deletes `session_path`, its retained `.wav` recording, and its `.dismissed`
/// marker iff the policy is [`RetentionPolicy::DiscardAfterDistill`], returning
/// whether the transcript was deleted. Called on the distill thread right after
/// a session distilled successfully, so nothing is reading the files. Refuses
/// any path that isn't a `.jsonl` file as a safety belt against ever deleting
/// the wrong thing. A path already gone (e.g. deleted by a concurrent sweep) is
/// `Ok(false)`, not an error, and a missing sibling (audio never retained, the
/// session never dismissed, or either already discarded) is the normal case —
/// both sibling deletes tolerate it.
pub fn apply_post_distill(policy: RetentionPolicy, session_path: &Path) -> io::Result<bool> {
    if policy != RetentionPolicy::DiscardAfterDistill {
        return Ok(false);
    }
    if session_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Ok(false);
    }
    let deleted = match fs::remove_file(session_path) {
        Ok(()) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(err) => return Err(err),
    };
    // The recording pairs with the transcript, so the policy that discards one
    // discards the other — even when the transcript was already gone, a
    // lingering recording under DiscardAfterDistill is a leak, not a keeper.
    match fs::remove_file(naming::audio_sibling(session_path)) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    // Same for the dismissed marker: a discarded session has nothing left to
    // wave off. (Usually already cleared by the distill success path; this
    // keeps "discard removes every session artifact" true for any caller.)
    match fs::remove_file(naming::dismissed_sibling(session_path)) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    Ok(deleted)
}

/// Whether a session file is older than `cutoff`. Dates the session from its
/// filename timestamp (the authoritative capture instant), falling back to the
/// file's mtime for a hand-imported name that doesn't match the scheme — the
/// same fallback the distill pass uses to recover a meeting date. A session
/// that can't be dated at all is never expired: retention must not delete what
/// it can't age.
fn is_expired(file_name: &str, mtime: Option<SystemTime>, cutoff: DateTime<Utc>) -> bool {
    match session_time(file_name, mtime) {
        Some(captured_at) => captured_at < cutoff,
        None => false,
    }
}

/// When a session was captured: its filename timestamp, else the file's mtime.
/// `None` when neither is available. Shared with
/// [`crate::sessions`] so "when was this session captured" has one definition —
/// the age the sweep prunes by and the timestamp the needs-attention list shows
/// can never disagree.
pub(crate) fn session_time(file_name: &str, mtime: Option<SystemTime>) -> Option<DateTime<Utc>> {
    naming::parse_session_filename(file_name)
        .and_then(|parsed| naming::parse_session_timestamp(&parsed.timestamp))
        .or_else(|| mtime.map(DateTime::<Utc>::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceId;
    use crate::settings::RetentionPolicy;
    use chrono::TimeZone;
    use std::num::NonZeroU32;
    use std::path::PathBuf;
    use std::time::Duration;

    fn device() -> DeviceId {
        DeviceId::parse("k4m2xp7q").unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap()
    }

    fn keep_days(days: u32) -> RetentionPolicy {
        RetentionPolicy::KeepDays {
            days: NonZeroU32::new(days).unwrap(),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut rand = [0u8; 4];
        getrandom::getrandom(&mut rand).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "kodabi-retention-{tag}-{}-{:08x}",
            std::process::id(),
            u32::from_le_bytes(rand)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes an empty session file named for `captured_at` and returns its path.
    fn seed_session(dir: &Path, captured_at: DateTime<Utc>) -> PathBuf {
        let name = naming::session_filename(captured_at, &device(), None, "jsonl");
        let path = dir.join(name);
        fs::write(&path, "").unwrap();
        path
    }

    #[test]
    fn keep_all_and_discard_sweeps_delete_nothing() {
        let dir = temp_dir("noop-sweep");
        let old = seed_session(&dir, now() - chrono::Duration::days(400));

        for policy in [
            RetentionPolicy::KeepAll,
            RetentionPolicy::DiscardAfterDistill,
        ] {
            let report = prune_sessions(&dir, policy, now()).unwrap();
            assert_eq!(report, PruneReport::default());
            assert!(old.exists());
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_days_deletes_only_expired_sessions() {
        let dir = temp_dir("keep-days");
        let old = seed_session(&dir, now() - chrono::Duration::days(60));
        let fresh = seed_session(&dir, now() - chrono::Duration::days(1));

        let report = prune_sessions(&dir, keep_days(7), now()).unwrap();

        assert_eq!(report.deleted, vec![old.clone()]);
        assert!(report.failed.is_empty());
        assert!(!old.exists(), "expired session should be pruned");
        assert!(fresh.exists(), "in-window session should be kept");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_days_prunes_expired_recordings_alongside_transcripts() {
        let dir = temp_dir("keep-days-wav");
        let old = seed_session(&dir, now() - chrono::Duration::days(60));
        let old_wav = naming::audio_sibling(&old);
        fs::write(&old_wav, "").unwrap();
        let fresh = seed_session(&dir, now() - chrono::Duration::days(1));
        let fresh_wav = naming::audio_sibling(&fresh);
        fs::write(&fresh_wav, "").unwrap();

        let report = prune_sessions(&dir, keep_days(7), now()).unwrap();

        assert!(report.failed.is_empty());
        assert_eq!(
            report.deleted.len(),
            2,
            "transcript and recording expire as a pair"
        );
        assert!(!old.exists());
        assert!(!old_wav.exists(), "expired recording should be pruned");
        assert!(fresh.exists());
        assert!(fresh_wav.exists(), "in-window recording should be kept");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_days_prunes_the_dismissed_marker_with_its_session() {
        let dir = temp_dir("keep-days-dismissed");
        let old = seed_session(&dir, now() - chrono::Duration::days(60));
        let old_marker = naming::dismissed_sibling(&old);
        fs::write(&old_marker, "2026-05-13T14:03:35Z").unwrap();
        let fresh = seed_session(&dir, now() - chrono::Duration::days(1));
        let fresh_marker = naming::dismissed_sibling(&fresh);
        fs::write(&fresh_marker, "2026-07-11T14:03:35Z").unwrap();
        // A marker orphaned by a hand-deleted transcript still expires: the
        // sweep is what reaps it.
        let orphan = seed_session(&dir, now() - chrono::Duration::days(90));
        let orphan_marker = naming::dismissed_sibling(&orphan);
        fs::write(&orphan_marker, "2026-04-13T14:03:35Z").unwrap();
        fs::remove_file(&orphan).unwrap();

        let report = prune_sessions(&dir, keep_days(7), now()).unwrap();

        assert!(report.failed.is_empty());
        assert!(!old.exists());
        assert!(
            !old_marker.exists(),
            "expired marker expires with its session"
        );
        assert!(!orphan_marker.exists(), "expired orphaned marker is reaped");
        assert!(fresh.exists());
        assert!(fresh_marker.exists(), "in-window marker should be kept");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sweep_ignores_non_jsonl_scratch_and_subdirectories() {
        let dir = temp_dir("selective");
        // A non-session file, a scratch temp (the shape write_raw_session
        // stages), and a subdirectory — none of which the sweep may touch.
        let notes = dir.join("notes.txt");
        fs::write(&notes, "keep me").unwrap();
        let scratch = dir.join(".raw-session.4821.0.tmp");
        fs::write(&scratch, "").unwrap();
        let subdir = dir.join("nested");
        fs::create_dir(&subdir).unwrap();
        // Even an "expired"-named .jsonl inside the subdir must be untouched.
        seed_session(&subdir, now() - chrono::Duration::days(400));

        let report = prune_sessions(&dir, keep_days(7), now()).unwrap();

        assert!(report.deleted.is_empty());
        assert!(notes.exists());
        assert!(scratch.exists());
        assert!(subdir.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sweep_leaves_the_inflight_subdirectory_untouched() {
        // The in-flight spill layout lives in sessions/inflight/. Retention's
        // claim is top-level `.jsonl` only, so a full in-flight session — even
        // one whose directory name dates it well past the cutoff — must survive
        // a sweep untouched. Guards the interaction with `crate::inflight`.
        let dir = temp_dir("inflight-untouched");
        let inflight = dir.join(crate::inflight::INFLIGHT_DIR);
        let session_dir = inflight.join("20250101T000000000Z-k4m2xp7q");
        fs::create_dir_all(&session_dir).unwrap();
        fs::write(session_dir.join(crate::inflight::META_FILE), "{}").unwrap();
        fs::write(session_dir.join(crate::inflight::MIC_PCM_FILE), [0u8; 16]).unwrap();
        fs::write(
            session_dir.join(crate::inflight::SYSTEM_PCM_FILE),
            [0u8; 16],
        )
        .unwrap();
        // A genuinely expired top-level session, to prove the sweep still runs.
        let expired = seed_session(&dir, now() - chrono::Duration::days(400));

        let report = prune_sessions(&dir, keep_days(7), now()).unwrap();

        assert_eq!(report.deleted, vec![expired]);
        assert!(session_dir.exists(), "in-flight session must be untouched");
        assert!(session_dir.join(crate::inflight::MIC_PCM_FILE).exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_sessions_dir_is_a_noop() {
        let dir = temp_dir("missing");
        let missing = dir.join("does-not-exist");
        let report = prune_sessions(&missing, keep_days(7), now()).unwrap();
        assert_eq!(report, PruneReport::default());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn is_expired_falls_back_to_mtime_for_unparseable_names() {
        let cutoff = now() - chrono::Duration::days(7);
        // A name that doesn't match the session scheme: dated by mtime instead.
        let old_mtime = SystemTime::from(now() - chrono::Duration::days(30));
        let fresh_mtime = SystemTime::from(now() - chrono::Duration::days(1));

        assert!(is_expired("hand-imported.jsonl", Some(old_mtime), cutoff));
        assert!(!is_expired(
            "hand-imported.jsonl",
            Some(fresh_mtime),
            cutoff
        ));
        // No timestamp and no mtime — never expired, never deleted.
        assert!(!is_expired("hand-imported.jsonl", None, cutoff));
    }

    #[test]
    fn is_expired_prefers_the_filename_timestamp_over_mtime() {
        let cutoff = now() - chrono::Duration::days(7);
        let expired_name =
            naming::session_filename(now() - chrono::Duration::days(30), &device(), None, "jsonl");
        // A misleading recent mtime must not save a session the filename dates as old.
        let recent_mtime = SystemTime::now() + Duration::from_secs(1);
        assert!(is_expired(&expired_name, Some(recent_mtime), cutoff));
    }

    #[test]
    fn apply_post_distill_deletes_only_under_discard_policy() {
        let dir = temp_dir("post-distill");
        let session = seed_session(&dir, now());
        let recording = naming::audio_sibling(&session);
        fs::write(&recording, "").unwrap();
        let marker = naming::dismissed_sibling(&session);
        fs::write(&marker, "2026-07-12T14:03:35Z").unwrap();

        // KeepAll / KeepDays leave the session and its siblings in place.
        assert!(!apply_post_distill(RetentionPolicy::KeepAll, &session).unwrap());
        assert!(!apply_post_distill(keep_days(30), &session).unwrap());
        assert!(session.exists());
        assert!(recording.exists());
        assert!(marker.exists());

        // DiscardAfterDistill removes all three.
        assert!(apply_post_distill(RetentionPolicy::DiscardAfterDistill, &session).unwrap());
        assert!(!session.exists());
        assert!(
            !recording.exists(),
            "the recording is discarded with its transcript"
        );
        assert!(
            !marker.exists(),
            "the dismissed marker is discarded with its transcript"
        );
        // Already-gone (both files) is Ok(false), not an error.
        assert!(!apply_post_distill(RetentionPolicy::DiscardAfterDistill, &session).unwrap());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apply_post_distill_tolerates_a_missing_recording() {
        // Audio may never have been retained (WAV write failed, or a pre-audio
        // session): the transcript still discards cleanly on its own.
        let dir = temp_dir("post-distill-no-wav");
        let session = seed_session(&dir, now());

        assert!(apply_post_distill(RetentionPolicy::DiscardAfterDistill, &session).unwrap());
        assert!(!session.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn apply_post_distill_refuses_a_non_jsonl_path() {
        let dir = temp_dir("post-distill-guard");
        let note = dir.join("note.md");
        fs::write(&note, "keep me").unwrap();

        assert!(!apply_post_distill(RetentionPolicy::DiscardAfterDistill, &note).unwrap());
        assert!(note.exists(), "a non-session path must never be deleted");

        fs::remove_dir_all(&dir).unwrap();
    }
}
