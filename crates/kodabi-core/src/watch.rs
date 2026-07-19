//! Watching the vault folder for note changes, debounced into a single
//! "something changed" signal.
//!
//! The index is kept live by re-running [`crate::reconcile::reconcile`] whenever
//! the vault changes on disk. This module owns the OS watcher and the debounce
//! that collapses a burst of raw filesystem events (a save is several: the
//! scratch write, the rename, an attribute touch) into one callback. It does not
//! interpret individual events — reconcile is idempotent and converges the whole
//! index — it only decides *whether* a burst is worth reconciling and *when* the
//! burst has settled.
//!
//! Relevance filtering keeps the app's own churn from looping: the index
//! database (`index.db`, `-wal`, `-shm`), transcription artifacts under
//! `sessions/`, WebView data, editor scratch files, and hidden/infra folders are
//! all ignored, so a reconcile's own writes never trigger another reconcile.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::note::RESERVED_ROOT_DIRS;
use crate::vault;

/// How long the vault must be quiet before a burst of changes is considered
/// settled. Longer than the note writer's scratch-then-rename sequence, so one
/// save is one reconcile.
pub const DEBOUNCE_QUIET: Duration = Duration::from_millis(500);

/// The longest a continuous stream of changes (a sync tool rewriting many files)
/// may delay a reconcile. Once this elapses since the burst began, reconcile
/// runs even if the vault has not gone quiet, so a steady writer can't starve it.
pub const DEBOUNCE_MAX_DELAY: Duration = Duration::from_secs(5);

/// A running vault watcher. Dropping it stops the OS watch and disconnects the
/// event channel, which ends the debounce thread.
pub struct VaultWatcher {
    _watcher: RecommendedWatcher,
}

/// A failure starting the vault watcher.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("failed to watch the vault: {0}")]
    Notify(#[from] notify::Error),
}

/// Watches `vault_root` recursively, invoking `on_change` once per debounced
/// burst of relevant (`.md`, non-reserved) changes. The returned [`VaultWatcher`]
/// must be kept alive for watching to continue.
pub fn watch_vault(
    vault_root: &Path,
    on_change: impl Fn() + Send + 'static,
) -> Result<VaultWatcher, WatchError> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        // The debounce thread owns interpretation; here we only forward. A send
        // error means that thread is gone (watcher dropped) — nothing to do.
        let _ = tx.send(res);
    })?;
    watcher.watch(vault_root, RecursiveMode::Recursive)?;

    let root = vault_root.to_path_buf();
    std::thread::spawn(move || {
        run_debounce(rx, root, DEBOUNCE_QUIET, DEBOUNCE_MAX_DELAY, on_change)
    });
    Ok(VaultWatcher { _watcher: watcher })
}

/// The debounce loop: idle until a relevant event, coalesce the burst, then fire
/// `on_change` once. Parameterized on the durations and channel so tests drive it
/// with synthetic events and short timings, no real watcher required.
fn run_debounce(
    rx: Receiver<notify::Result<Event>>,
    vault_root: PathBuf,
    quiet: Duration,
    max_delay: Duration,
    on_change: impl Fn(),
) {
    loop {
        // Idle: block until the next event. An irrelevant event while idle is
        // ignored outright — it must not start a burst.
        match rx.recv() {
            Ok(event) if event_matters(&vault_root, &event) => {}
            Ok(_) => continue,
            Err(_) => return, // sender dropped — the watcher is gone.
        }

        // A relevant event opened a burst. Coalesce until the vault goes quiet
        // for `quiet`, or `max_delay` elapses since the burst began.
        let burst_start = Instant::now();
        let mut quiet_deadline = burst_start + quiet;
        let max_deadline = burst_start + max_delay;
        loop {
            let now = Instant::now();
            let deadline = quiet_deadline.min(max_deadline);
            if now >= deadline {
                break;
            }
            match rx.recv_timeout(deadline - now) {
                Ok(event) => {
                    // Only a relevant event extends the quiet window; app churn
                    // (index writes, editor swap files) must not keep it open.
                    if event_matters(&vault_root, &event) {
                        quiet_deadline = Instant::now() + quiet;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                // Sender dropped mid-burst: fire for the pending change, then the
                // outer `recv` will observe the disconnect and return.
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        on_change();
    }
}

/// Whether a raw watcher result should trigger a reconcile. A watch error (e.g.
/// a buffer overflow that may have dropped events) conservatively triggers one,
/// since reconcile converges the whole index regardless.
fn event_matters(vault_root: &Path, result: &notify::Result<Event>) -> bool {
    match result {
        Ok(event) => event_is_relevant(vault_root, event),
        Err(_) => true,
    }
}

/// Whether an event touches an indexable note. A rescan-flagged or pathless event
/// (the backend lost track) triggers a reconcile to be safe; otherwise any path
/// that is a relevant `.md` file makes the event relevant.
fn event_is_relevant(vault_root: &Path, event: &Event) -> bool {
    if event.need_rescan() || event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|path| is_relevant(vault_root, path))
}

/// Whether a single path is an indexable note: a `.md` file under the vault, not
/// in the index/transcription/WebView reserved roots and not inside a hidden or
/// infrastructure (`.`/`_`-prefixed) folder. Purely lexical — no disk access.
fn is_relevant(vault_root: &Path, path: &Path) -> bool {
    if !vault::is_md_file(path) {
        return false;
    }
    let Ok(rel) = path.strip_prefix(vault_root) else {
        return false; // outside the vault (or the vault root itself)
    };

    // The trailing component is the filename; every earlier component is a
    // directory segment that must be listable.
    let segments: Vec<&std::ffi::OsStr> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let Some((_file, dirs)) = segments.split_last() else {
        return false;
    };
    for (depth, segment) in dirs.iter().enumerate() {
        let Some(name) = segment.to_str() else {
            return false;
        };
        if name.starts_with('.') || name.starts_with('_') {
            return false;
        }
        // Reserved roots (sessions/, raw/, EBWebView/) are only reserved at the
        // top level; deeper segments of the same name are legal project folders.
        // Note the Inbox is *not* reserved here — Inbox notes are indexable.
        if depth == 0 && is_reserved_root(name) {
            return false;
        }
    }
    true
}

/// Whether a top-level directory name is one of the reserved roots that never
/// holds indexable notes (case-insensitive, like the filesystem on Windows).
fn is_reserved_root(name: &str) -> bool {
    RESERVED_ROOT_DIRS
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::Sender;
    use std::sync::Arc;

    const VAULT: &str = if cfg!(windows) { r"C:\vault" } else { "/vault" };

    fn abs(rel: &str) -> PathBuf {
        Path::new(VAULT).join(rel)
    }

    #[test]
    fn is_relevant_accepts_notes_and_rejects_infra_and_reserved() {
        let root = Path::new(VAULT);
        let cases = [
            ("Ops/note.md", true),
            ("Inbox/idea.md", true),
            ("Growth/Q3/nested.md", true),
            ("root-level.md", true),
            ("Ops/.note.4821.0.tmp", false), // editor/writer scratch (not .md)
            ("sessions/decoy.md", false),    // reserved root
            ("raw/artifact.md", false),      // reserved root
            ("EBWebView/cache.md", false),   // reserved root
            ("Data/raw/deep.md", true),      // 'raw' only reserved at the top
            (".obsidian/config.md", false),  // hidden folder
            ("_scratch/wip.md", false),      // infra folder
            ("index.db-wal", false),         // not a .md file
            ("sessions/log.jsonl", false),   // not a .md file
        ];
        for (rel, want) in cases {
            assert_eq!(
                is_relevant(root, &abs(rel)),
                want,
                "is_relevant mismatch for {rel}"
            );
        }
    }

    #[test]
    fn is_relevant_rejects_paths_outside_the_vault() {
        let root = Path::new(VAULT);
        let outside = Path::new(if cfg!(windows) {
            r"C:\other\a.md"
        } else {
            "/other/a.md"
        });
        assert!(!is_relevant(root, outside));
    }

    /// A relevant `.md` event.
    fn relevant() -> notify::Result<Event> {
        Ok(Event::new(notify::EventKind::Any).add_path(abs("Ops/note.md")))
    }

    /// An irrelevant event (index WAL churn).
    fn irrelevant() -> notify::Result<Event> {
        Ok(Event::new(notify::EventKind::Any).add_path(abs("index.db-wal")))
    }

    /// Runs `run_debounce` on its own thread, feeds it via `feed`, then drops the
    /// sender and joins. Returns how many times `on_change` fired.
    fn drive(feed: impl FnOnce(&Sender<notify::Result<Event>>)) -> usize {
        let (tx, rx) = mpsc::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let thread_count = Arc::clone(&count);
        let handle = std::thread::spawn(move || {
            run_debounce(
                rx,
                PathBuf::from(VAULT),
                Duration::from_millis(20),
                Duration::from_millis(500),
                move || {
                    thread_count.fetch_add(1, Ordering::SeqCst);
                },
            );
        });
        feed(&tx);
        drop(tx);
        handle.join().unwrap();
        count.load(Ordering::SeqCst)
    }

    #[test]
    fn a_burst_of_events_fires_once() {
        let fired = drive(|tx| {
            for _ in 0..5 {
                tx.send(relevant()).unwrap();
            }
        });
        assert_eq!(fired, 1);
    }

    #[test]
    fn irrelevant_events_never_fire() {
        let fired = drive(|tx| {
            for _ in 0..5 {
                tx.send(irrelevant()).unwrap();
            }
        });
        assert_eq!(fired, 0);
    }

    #[test]
    fn two_bursts_separated_by_quiet_fire_twice() {
        let fired = drive(|tx| {
            tx.send(relevant()).unwrap();
            // Sleep well past the 20ms quiet window so the first burst settles.
            std::thread::sleep(Duration::from_millis(120));
            tx.send(relevant()).unwrap();
        });
        assert_eq!(fired, 2);
    }

    #[test]
    fn a_continuous_stream_fires_at_the_max_delay() {
        // Events every 15ms keep resetting the 40ms quiet window, so without a
        // max-delay cap the reconcile would never fire. The 70ms cap forces it.
        let (tx, rx) = mpsc::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let thread_count = Arc::clone(&count);
        let handle = std::thread::spawn(move || {
            run_debounce(
                rx,
                PathBuf::from(VAULT),
                Duration::from_millis(40),
                Duration::from_millis(70),
                move || {
                    thread_count.fetch_add(1, Ordering::SeqCst);
                },
            );
        });

        let feeder = std::thread::spawn(move || {
            for _ in 0..14 {
                if tx.send(relevant()).is_err() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        });

        // By 150ms the 70ms cap must have fired at least once mid-stream, even
        // though the 40ms quiet window never elapsed.
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            count.load(Ordering::SeqCst) >= 1,
            "max-delay cap should have forced a reconcile during the stream"
        );

        feeder.join().unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn watch_vault_reports_a_real_md_write() {
        // The one end-to-end test over a real OS watcher: write a note under a
        // tempdir vault and expect the debounced callback. Generous timeout to
        // absorb watcher setup and platform latency.
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().to_path_buf();
        std::fs::create_dir(vault.join("Ops")).unwrap();

        let (tx, rx) = mpsc::channel();
        let _watcher = watch_vault(&vault, move || {
            let _ = tx.send(());
        })
        .unwrap();

        std::fs::write(vault.join("Ops").join("note.md"), "hello").unwrap();

        rx.recv_timeout(Duration::from_secs(10))
            .expect("watcher should report the .md write within the timeout");
    }
}
