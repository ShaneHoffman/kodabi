//! In-flight capture sessions: the on-disk directory a live capture spills its
//! aligned audio into, so a crash or kill mid-meeting can be recovered on the
//! next launch instead of losing everything.
//!
//! Layout, under `<sessions>/inflight/<timestamp>-<device>/`:
//!
//! - `meta.json` — [`InflightMeta`]: schema version, sample rate, format, start
//!   instant (the recovered session's `captured_at`), device id, owning pid.
//! - `mic.f32le` — the "you" channel's raw little-endian `f32` PCM.
//! - `system.f32le` — the "them" channel's raw little-endian `f32` PCM.
//! - `lock` — held open exclusively for the session's whole life, so a
//!   still-running session in another instance is never mistaken for a
//!   recoverable orphan.
//!
//! **Liveness by lock, not by presence.** Kodabi has no single-instance guard,
//! so "a directory is here at startup" does not mean it was abandoned. The
//! owner opens `lock` with an exclusive share mode and keeps the handle; the OS
//! releases it the instant the process dies (including `kill -9`). Recovery
//! only claims a directory whose lock it can acquire — a directory a live
//! sibling still holds is skipped, and acquiring the lock also serialises two
//! concurrent recoverers.
//!
//! **Retention leaves this alone.** `<sessions>/inflight/` is a subdirectory,
//! and [`crate::retention::prune_sessions`] only ever touches top-level
//! `.jsonl` files — so the spill format sits outside its deletion claim by
//! construction. Cleanup of un-recoverable directories (corrupt metadata, a
//! mis-tap, a lone channel) is this module's own [`sweep_stale`], run on the
//! same schedule.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::device::DeviceId;
use crate::naming;

/// Subdirectory of the sessions dir that holds in-flight captures.
pub const INFLIGHT_DIR: &str = "inflight";
pub const META_FILE: &str = "meta.json";
pub const MIC_PCM_FILE: &str = "mic.f32le";
pub const SYSTEM_PCM_FILE: &str = "system.f32le";
pub const LOCK_FILE: &str = "lock";

/// Scratch name for the atomic metadata write (`.meta.tmp` → rename to
/// `meta.json`), so a crash mid-write can never leave a half-written
/// `meta.json` for recovery to choke on.
const META_TMP_FILE: &str = ".meta.tmp";

/// The only metadata schema version this build understands. A directory with
/// any other version is treated as un-recoverable (discarded when stale)
/// rather than misread.
const META_VERSION: u32 = 1;

/// The spill sample format recorded in metadata: little-endian `f32`.
const SPILL_FORMAT: &str = "f32le";

/// Metadata written alongside an in-flight session's spill files. Its exact
/// JSON shape is locked by a test — recovery on a later build reads it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InflightMeta {
    /// Schema version; see [`META_VERSION`].
    pub version: u32,
    /// The combiner's aligned sample rate (48 kHz), for interpreting the PCM.
    pub sample_rate: u32,
    /// Sample encoding of the `.f32le` files. Recorded so a later switch to a
    /// smaller on-disk format is a version bump, not a silent reinterpretation.
    pub format: String,
    /// When capture started — the recovered session's `captured_at`, more
    /// accurate than the stop-time back-dating a normal completion uses.
    pub started_at: DateTime<Utc>,
    /// The capturing device's identity, for the recovered session's filename.
    pub device_id: String,
    /// The process that owns this session. Informational only — the `lock`
    /// file, not this pid, is the authoritative liveness signal.
    pub pid: u32,
}

/// A live, owned in-flight session: its directory, metadata, and the held
/// exclusive lock. Created at capture start and either finished-and-removed
/// after the session's transcript lands, or (on a crash) left for recovery.
pub struct InflightSession {
    dir: PathBuf,
    pub meta: InflightMeta,
    /// The exclusive lock, held for the session's whole life. Dropped (before
    /// the directory delete) by [`InflightSession::remove`]; also releases on
    /// its own `Drop` if the session is abandoned without removal.
    lock: File,
}

impl InflightSession {
    /// The session's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path the mic ("you") channel spills to.
    pub fn mic_path(&self) -> PathBuf {
        self.dir.join(MIC_PCM_FILE)
    }

    /// Path the system ("them") channel spills to.
    pub fn system_path(&self) -> PathBuf {
        self.dir.join(SYSTEM_PCM_FILE)
    }

    /// When this capture started (from metadata) — the session's `captured_at`.
    pub fn started_at(&self) -> DateTime<Utc> {
        self.meta.started_at
    }

    /// Release the lock and delete the whole directory. Called once the
    /// session's transcript has been written (its content is safe on disk), or
    /// when a mis-tap produced nothing worth keeping. The lock handle is
    /// dropped before the delete because Windows refuses to remove a directory
    /// containing an open file.
    pub fn remove(self) -> io::Result<()> {
        let InflightSession { dir, lock, .. } = self;
        drop(lock);
        fs::remove_dir_all(&dir)
    }
}

/// The in-flight root under a sessions directory (`<sessions>/inflight`).
pub fn inflight_root(sessions_dir: &Path) -> PathBuf {
    sessions_dir.join(INFLIGHT_DIR)
}

/// Create a fresh in-flight session under `sessions_dir`: make its directory,
/// claim the exclusive lock, and write metadata. The spill files themselves are
/// created by the audio combiner (handed [`InflightSession::mic_path`] /
/// [`InflightSession::system_path`]), not here.
pub fn create_session(
    sessions_dir: &Path,
    device: &DeviceId,
    started_at: DateTime<Utc>,
    sample_rate: u32,
) -> io::Result<InflightSession> {
    let root = inflight_root(sessions_dir);
    fs::create_dir_all(&root)?;

    let base = naming::session_dir_name(started_at, device);
    let dir = create_unique_dir(&root, &base)?;

    // Claim the lock before anything else, so the window in which the directory
    // exists without an owner is as small as possible.
    let lock = acquire_lock_file(&dir.join(LOCK_FILE), true)?;

    let meta = InflightMeta {
        version: META_VERSION,
        sample_rate,
        format: SPILL_FORMAT.to_string(),
        started_at,
        device_id: device.as_str().to_string(),
        pid: std::process::id(),
    };
    write_meta_atomic(&dir, &meta)?;

    Ok(InflightSession { dir, meta, lock })
}

/// A recoverable orphan: a directory that was abandoned (its lock is free), has
/// valid metadata, and holds enough audio in both channels to be worth
/// transcribing. Holds the lock so nothing else claims it mid-recovery.
pub struct RecoverableOrphan {
    pub dir: PathBuf,
    pub meta: InflightMeta,
    pub mic_pcm: PathBuf,
    pub system_pcm: PathBuf,
    /// Held for the recovery's duration so nothing else claims the directory;
    /// released by [`RecoverableOrphan::remove`] or on drop.
    lock: File,
}

impl RecoverableOrphan {
    /// Release the lock and delete the directory — called once the recovered
    /// session's transcript has landed. Mirrors [`InflightSession::remove`]:
    /// the lock handle is dropped first so Windows will remove the directory.
    pub fn remove(self) -> io::Result<()> {
        let RecoverableOrphan { dir, lock, .. } = self;
        drop(lock);
        fs::remove_dir_all(&dir)
    }
}

/// A scanned in-flight directory the caller should act on: recover it, or
/// discard it (a directory that is corrupt, un-versioned, or too short to be a
/// real meeting). Directories a live session still holds are omitted entirely.
pub enum OrphanKind {
    Recoverable(RecoverableOrphan),
    Discard { dir: PathBuf, reason: String },
}

/// Scan the in-flight root for orphaned sessions. Each directory is either
/// recoverable, discardable, or (skipped, not returned) still held by a live
/// session. A session is recoverable only if its lock is free, its metadata is
/// valid, and both channels hold at least `min_duration` of audio — the same
/// mis-tap / lone-source bar a normal stop applies.
///
/// A missing in-flight root is not an error (nothing has spilled yet): it
/// yields an empty list.
pub fn scan(inflight_root: &Path, min_duration: Duration) -> io::Result<Vec<OrphanKind>> {
    let entries = match fs::read_dir(inflight_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut orphans = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        match classify(&entry.path(), min_duration) {
            Classification::Live => {}
            Classification::Recoverable(orphan) => orphans.push(OrphanKind::Recoverable(orphan)),
            Classification::Discard { dir, reason } => {
                orphans.push(OrphanKind::Discard { dir, reason })
            }
        }
    }
    Ok(orphans)
}

/// Delete only in-flight directories that are (a) datable and older than
/// `grace`, (b) not held by a live session, and (c) not recoverable — i.e.
/// corrupt, un-versioned, lone-channel, or mis-tap leftovers. A recoverable
/// orphan is always kept (it is retried once per launch, never auto-deleted),
/// and an un-datable directory is kept rather than guessed at — mirroring
/// [`crate::retention`]'s conservatism. Returns the directories removed.
pub fn sweep_stale(
    inflight_root: &Path,
    now: DateTime<Utc>,
    grace: Duration,
    min_duration: Duration,
) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(inflight_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let grace = chrono::Duration::from_std(grace).unwrap_or_else(|_| chrono::Duration::days(2));
    let mut removed = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let dir = entry.path();
        let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
        // Never delete what we can't date, and never delete anything still
        // inside the grace window (a just-created live session included).
        let Some(started) = dir_time(&dir, mtime) else {
            continue;
        };
        if now - started < grace {
            continue;
        }
        // Only un-recoverable, unlocked directories are actually deleted.
        if let Classification::Discard { .. } = classify(&dir, min_duration) {
            if fs::remove_dir_all(&dir).is_ok() {
                removed.push(dir);
            }
        }
    }
    Ok(removed)
}

/// The three states a scanned in-flight directory can be in.
enum Classification {
    /// A live session holds the lock — leave it entirely alone.
    Live,
    Recoverable(RecoverableOrphan),
    Discard {
        dir: PathBuf,
        reason: String,
    },
}

fn classify(dir: &Path, min_duration: Duration) -> Classification {
    // The lock is the gate: acquire it and we own the (abandoned) directory;
    // fail to and a live session still has it.
    let lock = match acquire_lock(&dir.join(LOCK_FILE)) {
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::Contended => return Classification::Live,
        // No lock file at all: a directory that never finished being created,
        // or otherwise not a valid session. Nothing to recover.
        LockOutcome::Missing => {
            return Classification::Discard {
                dir: dir.to_path_buf(),
                reason: "no lock file".to_string(),
            }
        }
    };

    let meta = match read_meta(dir) {
        Ok(meta) => meta,
        Err(reason) => {
            return Classification::Discard {
                dir: dir.to_path_buf(),
                reason,
            }
        }
    };

    let mic_pcm = dir.join(MIC_PCM_FILE);
    let system_pcm = dir.join(SYSTEM_PCM_FILE);
    let min_samples = (min_duration.as_secs_f64() * meta.sample_rate as f64) as u64;
    if !has_min_samples(&mic_pcm, min_samples) || !has_min_samples(&system_pcm, min_samples) {
        return Classification::Discard {
            dir: dir.to_path_buf(),
            reason: "too little captured audio to be a meeting".to_string(),
        };
    }

    Classification::Recoverable(RecoverableOrphan {
        dir: dir.to_path_buf(),
        meta,
        mic_pcm,
        system_pcm,
        lock,
    })
}

/// Whether `path` holds at least `min_samples` whole `f32` samples.
fn has_min_samples(path: &Path, min_samples: u64) -> bool {
    fs::metadata(path)
        .map(|m| m.len() / 4 >= min_samples)
        .unwrap_or(false)
}

fn read_meta(dir: &Path) -> Result<InflightMeta, String> {
    let bytes =
        fs::read(dir.join(META_FILE)).map_err(|err| format!("unreadable metadata: {err}"))?;
    let meta: InflightMeta =
        serde_json::from_slice(&bytes).map_err(|err| format!("corrupt metadata: {err}"))?;
    if meta.version != META_VERSION {
        return Err(format!("unsupported metadata version {}", meta.version));
    }
    Ok(meta)
}

fn write_meta_atomic(dir: &Path, meta: &InflightMeta) -> io::Result<()> {
    let body = serde_json::to_vec(meta).map_err(io::Error::other)?;
    let tmp = dir.join(META_TMP_FILE);
    fs::write(&tmp, &body)?;
    fs::rename(&tmp, dir.join(META_FILE))
}

/// Recover the start instant a directory was named for (the part before the
/// first `-`), falling back to its mtime for a hand-renamed directory, and to
/// `None` when it can't be dated at all.
fn dir_time(dir: &Path, mtime: Option<SystemTime>) -> Option<DateTime<Utc>> {
    dir.file_name()
        .and_then(|n| n.to_str())
        .and_then(|name| name.split('-').next())
        .and_then(naming::parse_session_timestamp)
        .or_else(|| mtime.map(DateTime::<Utc>::from))
}

/// Create `<root>/<base>`, disambiguating a same-millisecond collision with a
/// `-2`, `-3`, … suffix (as [`crate::naming::numbered_slug`] does for files).
fn create_unique_dir(root: &Path, base: &str) -> io::Result<PathBuf> {
    match fs::create_dir(root.join(base)) {
        Ok(()) => return Ok(root.join(base)),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    for n in 2..1_000 {
        let candidate = root.join(format!("{base}-{n}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other(
        "too many in-flight session directory collisions",
    ))
}

/// The outcome of trying to acquire a directory's lock.
enum LockOutcome {
    Acquired(File),
    /// A live session holds it (a sharing violation, or otherwise unopenable).
    Contended,
    /// No lock file exists.
    Missing,
}

fn acquire_lock(lock_path: &Path) -> LockOutcome {
    match acquire_lock_file(lock_path, false) {
        Ok(file) => LockOutcome::Acquired(file),
        Err(err) if err.kind() == io::ErrorKind::NotFound => LockOutcome::Missing,
        // A sharing violation (a live owner) or any other open failure: treat
        // as contended and leave the directory strictly alone.
        Err(_) => LockOutcome::Contended,
    }
}

/// Open a lock file with an exclusive share mode, so that while this handle is
/// held no other process (or handle) can open it. On Windows this is a real OS
/// lock released on process death; elsewhere it degrades to an ordinary open
/// (the app ships on Windows, where the guarantee holds).
#[cfg(windows)]
fn acquire_lock_file(lock_path: &Path, create: bool) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    let mut options = File::options();
    options.read(true).write(true).share_mode(0);
    if create {
        options.create(true);
    }
    options.open(lock_path)
}

#[cfg(not(windows))]
fn acquire_lock_file(lock_path: &Path, create: bool) -> io::Result<File> {
    let mut options = File::options();
    options.read(true).write(true);
    if create {
        options.create(true);
    }
    options.open(lock_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};
    use tempfile::tempdir;

    fn device() -> DeviceId {
        DeviceId::parse("k4m2xp7q").unwrap()
    }

    fn instant(millis: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35)
            .unwrap()
            .with_nanosecond(millis * 1_000_000)
            .unwrap()
    }

    /// Write `samples` worth of zeroed `f32` PCM to a spill file.
    fn write_pcm(path: &Path, samples: usize) {
        fs::write(path, vec![0u8; samples * 4]).unwrap();
    }

    #[test]
    fn meta_json_wire_shape_is_stable() {
        let meta = InflightMeta {
            version: 1,
            sample_rate: 48_000,
            format: "f32le".to_string(),
            started_at: Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap(),
            device_id: "k4m2xp7q".to_string(),
            pid: 4821,
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert_eq!(
            json,
            r#"{"version":1,"sample_rate":48000,"format":"f32le","started_at":"2026-07-12T14:03:35Z","device_id":"k4m2xp7q","pid":4821}"#
        );
        // And it round-trips, subseconds included.
        let with_millis = InflightMeta {
            started_at: instant(123),
            ..meta
        };
        let round =
            serde_json::from_str::<InflightMeta>(&serde_json::to_string(&with_millis).unwrap())
                .unwrap();
        assert_eq!(round, with_millis);
    }

    #[test]
    fn create_session_lays_out_the_directory() {
        let sessions = tempdir().unwrap();
        let session = create_session(sessions.path(), &device(), instant(0), 48_000).unwrap();

        assert!(session.dir().join(META_FILE).exists());
        assert!(session.dir().join(LOCK_FILE).exists());
        assert_eq!(session.mic_path(), session.dir().join(MIC_PCM_FILE));
        assert_eq!(session.system_path(), session.dir().join(SYSTEM_PCM_FILE));
        assert_eq!(session.started_at(), instant(0));
        assert_eq!(session.meta.sample_rate, 48_000);
        assert_eq!(session.meta.format, "f32le");
        // The directory sits under sessions/inflight/, outside retention's claim.
        assert_eq!(
            session.dir().parent().unwrap(),
            inflight_root(sessions.path())
        );
    }

    #[test]
    fn same_instant_sessions_get_distinct_directories() {
        let sessions = tempdir().unwrap();
        let a = create_session(sessions.path(), &device(), instant(0), 48_000).unwrap();
        let b = create_session(sessions.path(), &device(), instant(0), 48_000).unwrap();
        assert_ne!(a.dir(), b.dir());
    }

    #[test]
    fn scan_recovers_an_abandoned_session() {
        let sessions = tempdir().unwrap();
        let session = create_session(sessions.path(), &device(), instant(0), 48_000).unwrap();
        // Enough audio in both channels to clear the 1ms bar used below.
        write_pcm(&session.mic_path(), 1_000);
        write_pcm(&session.system_path(), 1_000);
        let dir = session.dir().to_path_buf();
        // Simulate the owning process dying: drop the session, releasing the lock.
        drop(session);

        let orphans = scan(&inflight_root(sessions.path()), Duration::from_millis(1)).unwrap();
        assert_eq!(orphans.len(), 1);
        match &orphans[0] {
            OrphanKind::Recoverable(orphan) => {
                assert_eq!(orphan.dir, dir);
                assert_eq!(orphan.meta.started_at, instant(0));
            }
            OrphanKind::Discard { reason, .. } => panic!("expected recoverable, got {reason}"),
        }
    }

    #[test]
    fn scan_discards_a_corrupt_meta() {
        let sessions = tempdir().unwrap();
        let session = create_session(sessions.path(), &device(), instant(0), 48_000).unwrap();
        write_pcm(&session.mic_path(), 1_000);
        write_pcm(&session.system_path(), 1_000);
        let dir = session.dir().to_path_buf();
        drop(session);
        fs::write(dir.join(META_FILE), "{ not valid json").unwrap();

        let orphans = scan(&inflight_root(sessions.path()), Duration::from_millis(1)).unwrap();
        assert_eq!(orphans.len(), 1);
        assert!(matches!(orphans[0], OrphanKind::Discard { .. }));
    }

    #[test]
    fn scan_discards_a_lone_channel_session() {
        let sessions = tempdir().unwrap();
        let session = create_session(sessions.path(), &device(), instant(0), 48_000).unwrap();
        // Mic captured, system stayed empty (never attached / crashed early).
        write_pcm(&session.mic_path(), 1_000);
        write_pcm(&session.system_path(), 0);
        drop(session);

        let orphans = scan(&inflight_root(sessions.path()), Duration::from_millis(1)).unwrap();
        assert_eq!(orphans.len(), 1);
        assert!(matches!(orphans[0], OrphanKind::Discard { .. }));
    }

    #[test]
    #[cfg(windows)]
    fn scan_skips_a_live_locked_session() {
        let sessions = tempdir().unwrap();
        // Keep the session alive (lock held) — recovery must not touch it.
        let session = create_session(sessions.path(), &device(), instant(0), 48_000).unwrap();
        write_pcm(&session.mic_path(), 1_000);
        write_pcm(&session.system_path(), 1_000);

        let orphans = scan(&inflight_root(sessions.path()), Duration::from_millis(1)).unwrap();
        assert!(orphans.is_empty(), "a live session must not be recovered");
        drop(session);
    }

    #[test]
    fn sweep_stale_removes_old_discardable_but_keeps_recoverable_and_recent() {
        let sessions = tempdir().unwrap();
        let root = inflight_root(sessions.path());
        let now = instant(0);

        // 1) An old, corrupt (discardable) session → removed.
        let old_corrupt = create_session(
            sessions.path(),
            &device(),
            now - chrono::Duration::days(10),
            48_000,
        )
        .unwrap();
        let old_corrupt_dir = old_corrupt.dir().to_path_buf();
        drop(old_corrupt);
        fs::write(old_corrupt_dir.join(META_FILE), "garbage").unwrap();

        // 2) An old but recoverable session → kept (retried, not deleted).
        let old_good = create_session(
            sessions.path(),
            &device(),
            now - chrono::Duration::days(10),
            48_000,
        )
        .unwrap();
        write_pcm(&old_good.mic_path(), 1_000);
        write_pcm(&old_good.system_path(), 1_000);
        let old_good_dir = old_good.dir().to_path_buf();
        drop(old_good);

        // 3) A recent discardable session → kept (inside the grace window).
        let recent_corrupt = create_session(
            sessions.path(),
            &device(),
            now - chrono::Duration::hours(1),
            48_000,
        )
        .unwrap();
        let recent_corrupt_dir = recent_corrupt.dir().to_path_buf();
        drop(recent_corrupt);
        fs::write(recent_corrupt_dir.join(META_FILE), "garbage").unwrap();

        let removed = sweep_stale(
            &root,
            now,
            Duration::from_secs(48 * 3600),
            Duration::from_millis(1),
        )
        .unwrap();

        assert_eq!(removed, vec![old_corrupt_dir.clone()]);
        assert!(!old_corrupt_dir.exists());
        assert!(old_good_dir.exists(), "recoverable session must be retried");
        assert!(
            recent_corrupt_dir.exists(),
            "recent session is inside grace"
        );
    }

    #[test]
    fn scan_of_a_missing_root_is_empty() {
        let sessions = tempdir().unwrap();
        let orphans = scan(&inflight_root(sessions.path()), Duration::from_secs(1)).unwrap();
        assert!(orphans.is_empty());
    }
}
