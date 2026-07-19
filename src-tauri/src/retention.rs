//! Shell wiring for retention pruning: resolves the sessions directory and the
//! current policy, then runs the pure `kodabi_core::retention` sweep. All the
//! deletion logic lives in the core crate; this module only owns the schedule
//! (a background thread) and the `AppHandle`-bound plumbing.

use std::time::Duration;

use kodabi_core::retention::prune_sessions;
use tauri::{AppHandle, Emitter, Manager};

use crate::settings_cmds::SettingsState;
use crate::transcribe::knowledge_base_dir;

/// How often the background sweep re-checks for expired sessions. Retention is
/// measured in days, so an hours-scale cadence is ample — a session that ages
/// past the cutoff at worst lingers a few extra hours before the next pass.
const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Runs one prune pass on a background thread and returns immediately, so no
/// caller (a policy change over IPC, startup setup) ever blocks on filesystem
/// work. Resolves the sessions directory and policy fresh each time.
pub(crate) fn spawn_prune(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || prune_once(&app));
}

/// Starts the retention schedule: an immediate sweep, then one every
/// [`SWEEP_INTERVAL`]. Re-reads the policy each pass, so a policy the user
/// changes mid-session takes effect on the next sweep without a restart. The
/// thread is detached and dies with the process.
pub(crate) fn start_schedule(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        prune_once(&app);
        std::thread::sleep(SWEEP_INTERVAL);
    });
}

/// One synchronous prune pass. Best-effort: a missing settings state (very
/// early startup) or an unresolvable KB dir just skips this pass, and per-file
/// failures are reported by the core sweep, logged here, and never fatal.
fn prune_once(app: &AppHandle) {
    let Some(state) = app.try_state::<SettingsState>() else {
        return;
    };
    let policy = state.snapshot().retention;

    let kb_dir = match knowledge_base_dir(app) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("retention prune skipped: {err}");
            return;
        }
    };
    let sessions_dir = kb_dir.join("sessions");

    match prune_sessions(&sessions_dir, policy, chrono::Utc::now()) {
        Ok(report) => {
            for path in &report.failed {
                eprintln!("retention: failed to prune {}", path.display());
            }
            // Announce only a sweep that actually removed something: a
            // needs-attention list on screen may be showing a session this pass
            // just deleted, and it should drop the row rather than offer a retry
            // that would fail on a missing file.
            if !report.deleted.is_empty() {
                let _ = app.emit(crate::events::SESSIONS_CHANGED_EVENT, ());
            }
        }
        Err(err) => eprintln!("retention prune failed: {err}"),
    }

    // Piggyback the in-flight sweep on this cadence: delete only un-recoverable
    // spill directories that are safely past the grace window (recoverable ones
    // are retried at launch, never swept). Independent of the retention policy —
    // an in-flight leftover is process wreckage, not a kept session.
    let inflight_root = kodabi_core::inflight::inflight_root(&sessions_dir);
    if let Err(err) = kodabi_core::inflight::sweep_stale(
        &inflight_root,
        chrono::Utc::now(),
        crate::transcribe::INFLIGHT_STALE_GRACE,
        crate::transcribe::MIN_TRANSCRIBE_DURATION,
    ) {
        eprintln!("retention: in-flight sweep failed: {err}");
    }
}
