//! Teardown the updater's own exit path skips.
//!
//! `tauri-plugin-updater`'s Windows install does not go out through the normal
//! door: it calls `cleanup_before_exit()` and then `std::process::exit(0)`, so
//! the `RunEvent::ExitRequested | Exit` arm in [`crate::run`] — the one place
//! the terminal and chat child processes are reaped — never runs. Left alone,
//! the `claude` process tree survives the app, and the `kodabi-mcp.exe` inside
//! it holds a Windows image lock on a file the NSIS installer is about to
//! overwrite. The installer then stalls on a retry prompt in the middle of what
//! is supposed to be a passive update.
//!
//! So the frontend calls this immediately before `Update.install()`
//! (`src/useUpdater.ts`), which is as late as the reap can happen and still be
//! sure of running. There is no plugin hook to hang it on instead: the JS
//! install path offers no `on_before_exit`.
//!
//! Nothing here belongs in `kodabi-core` — reaping shell-owned child processes
//! off an `AppHandle` is the definition of shell work, and core cannot depend
//! on tauri.

use tauri::AppHandle;

/// Reaps the child process trees before the updater replaces the binary.
///
/// Infallible by construction: both reaps no-op when nothing is running, and a
/// failure to tear down is not a reason to block an update the user asked for.
#[tauri::command]
pub fn updater_prepare_install(app: AppHandle) {
    crate::terminal_cmds::reap(&app);
    crate::chat_cmds::reap(&app);
}
