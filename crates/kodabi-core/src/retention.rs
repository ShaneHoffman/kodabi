//! Pruning raw session transcripts per the user's [`RetentionPolicy`].
//!
//! Sessions are the `.jsonl` files [`crate::raw_session::write_raw_session`]
//! drops in the KB's `sessions/` directory. This module enforces the retention
//! policy over them two ways:
//!
//! - [`prune_sessions`] — a sweep, for [`RetentionPolicy::KeepDays`]: delete
//!   sessions older than the cutoff. Run on a schedule (startup + periodically)
//!   and whenever the policy changes.
//! - [`apply_post_distill`] — forward-only enforcement of
//!   [`RetentionPolicy::DiscardAfterDistill`]: delete a single session the
//!   moment it has been successfully distilled into a note.
//!
//! Deletion is deliberately conservative: only regular `.jsonl` files directly
//! in the sessions directory are ever touched (never the `.tmp` scratch files
//! an in-flight write stages, subdirectories, or anything else), and a session
//! whose age can't be determined is kept rather than guessed-at.

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
        // Only `.jsonl` session files; this also excludes the `.tmp` scratch
        // files an in-flight `write_raw_session` stages before its atomic link.
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
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

/// Deletes `session_path` iff the policy is
/// [`RetentionPolicy::DiscardAfterDistill`], returning whether it deleted.
/// Called on the distill thread right after a session distilled successfully,
/// so nothing is reading the file. Refuses any path that isn't a `.jsonl` file
/// as a safety belt against ever deleting the wrong thing. A path already gone
/// (e.g. deleted by a concurrent sweep) is `Ok(false)`, not an error.
pub fn apply_post_distill(policy: RetentionPolicy, session_path: &Path) -> io::Result<bool> {
    if policy != RetentionPolicy::DiscardAfterDistill {
        return Ok(false);
    }
    if session_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Ok(false);
    }
    match fs::remove_file(session_path) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
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

fn session_time(file_name: &str, mtime: Option<SystemTime>) -> Option<DateTime<Utc>> {
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

        // KeepAll / KeepDays leave the session in place.
        assert!(!apply_post_distill(RetentionPolicy::KeepAll, &session).unwrap());
        assert!(!apply_post_distill(keep_days(30), &session).unwrap());
        assert!(session.exists());

        // DiscardAfterDistill removes it.
        assert!(apply_post_distill(RetentionPolicy::DiscardAfterDistill, &session).unwrap());
        assert!(!session.exists());
        // Already-gone is Ok(false), not an error.
        assert!(!apply_post_distill(RetentionPolicy::DiscardAfterDistill, &session).unwrap());

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
