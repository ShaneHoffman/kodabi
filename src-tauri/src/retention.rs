//! Shell wiring for retention pruning: resolves the sessions directory and the
//! current policy, then runs the pure `kodabi_core::retention` sweep. All the
//! deletion logic lives in the core crate; this module only owns the schedule
//! (a background thread) and the `AppHandle`-bound plumbing.

use std::time::Duration;

use kodabi_core::retention::prune_sessions;
use tauri::{AppHandle, Manager};

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
        }
        Err(err) => eprintln!("retention prune failed: {err}"),
    }
}
