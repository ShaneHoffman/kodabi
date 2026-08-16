//! Dev sandbox activation — the shell around [`kodabi_core::sandbox`].
//!
//! Reads `KODABI_SANDBOX`, resolves every state path from it, refuses the
//! misuses the core module defines, and then does the one thing that makes the
//! rest of the app follow along: **rewrites the process environment**. Once
//! `KODABI_KB_ROOT` and `KODABI_INDEX_DB` name sandbox paths, every existing
//! resolver (`transcribe::knowledge_base_dir`, `index_state::index_db_path`),
//! the asset-protocol scope widening, the generated `.mcp.json`, and every
//! child process Kodabi spawns — the `kodabi-mcp` sidecar, headless Claude —
//! are already pointing at the sandbox with no further code changes.
//!
//! That leaves exactly one path the two env seams never covered:
//! `app_config_dir()`, home to `settings.toml`, `device.toml` and `_claude/`.
//! [`config_dir`] is that third seam, which `e2e/README.md` reserved for
//! "when something here needs to *write* config". A sandbox does: a preview
//! session flips retention policy and acknowledges consent, and doing that to
//! the developer's real settings is the hazard this module exists to remove.
//!
//! **Ordering.** [`activate`] must run as the first statement of
//! `run()` — before the Tauri builder, any plugin, any window, and any thread.
//! `WEBVIEW2_USER_DATA_FOLDER` is only read when a webview is created, and
//! `std::env::set_var` is only sound while the process is still
//! single-threaded (this crate is edition 2021, where the call is safe, but the
//! soundness requirement is the same).
//!
//! **Unset is untouched.** With `KODABI_SANDBOX` absent or empty this module
//! returns immediately, writing nothing and reading nothing else — a plain
//! `pnpm tauri dev` and every release build behave exactly as they did before
//! the sandbox existed. See `docs/DEV_SANDBOX.md`.

use std::path::PathBuf;
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};

use kodabi_core::sandbox::{self, RealAppDirs, SandboxEnv, SandboxPaths};

/// The active sandbox base, or `None` for a normal launch. Set once by
/// [`activate`]; read by [`config_dir`] and [`active`].
static SANDBOX_BASE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Environment variables activation writes so the rest of the app follows the
/// sandbox without knowing it exists.
const KB_ROOT_ENV: &str = "KODABI_KB_ROOT";
const INDEX_DB_ENV: &str = "KODABI_INDEX_DB";
const WEBVIEW2_ENV: &str = "WEBVIEW2_USER_DATA_FOLDER";
const DISABLE_CHAT_DISTILL_ENV: &str = "KODABI_DISABLE_CHAT_DISTILL";

/// Resolves and installs the dev sandbox, or exits.
///
/// `identifier` is the bundle identifier from the Tauri context, which is what
/// Tauri itself joins onto `dirs::data_dir()` / `dirs::config_dir()` to form
/// the real app directories — so the sandbox computes the very paths it must
/// avoid, rather than guessing them.
///
/// Refusals terminate the process with a message rather than falling back to
/// the real directories: a caller who asked for a sandbox and silently got real
/// data is the one outcome worth crashing over. Exiting (rather than returning
/// an error into the builder) keeps the failure legible at the point of launch,
/// where the harness and the dev:sandbox script both capture stderr.
pub(crate) fn activate(identifier: &str) {
    let Some(switch) = non_empty(sandbox::SANDBOX_ENV) else {
        // The overwhelmingly common path: no sandbox, nothing touched.
        let _ = SANDBOX_BASE.set(None);
        return;
    };

    let real = match real_app_dirs(identifier) {
        Ok(dirs) => dirs,
        Err(message) => refuse(&message),
    };
    let env = SandboxEnv {
        switch,
        explicit_kb_root: non_empty(KB_ROOT_ENV),
        explicit_index_db: non_empty(INDEX_DB_ENV),
    };

    let paths = match sandbox::resolve(&env, &real) {
        Ok(paths) => paths,
        Err(err) => refuse(&err.to_string()),
    };
    if let Err(err) = std::fs::create_dir_all(&paths.base) {
        refuse(&format!(
            "could not create the sandbox directory {:?}: {err}",
            paths.base
        ));
    }

    install(&paths);
    eprintln!(
        "kodabi: sandbox mode active. All state under {:?}",
        paths.base
    );
    let _ = SANDBOX_BASE.set(Some(paths.base));
}

/// Points the rest of the process at the sandbox.
///
/// The two vault seams are overwritten unconditionally — [`sandbox::resolve`]
/// has already refused the case where the caller set them, so there is nothing
/// here to clobber. The other two defer to an explicit value, because each has
/// a legitimate reason to be set from outside: a launcher that wants its own
/// WebView2 profile, and a developer re-enabling chat distill inside the
/// sandbox (`KODABI_DISABLE_CHAT_DISTILL=""`, which `distill_disabled` reads as
/// unset).
fn install(paths: &SandboxPaths) {
    std::env::set_var(KB_ROOT_ENV, &paths.base);
    std::env::set_var(INDEX_DB_ENV, &paths.index_db);

    if non_empty(WEBVIEW2_ENV).is_none() {
        // Tauri leaves this unset, so WebView2 derives the profile from the
        // executable name — meaning a sandboxed run would otherwise share a
        // browser process (and its localStorage) with a real instance of the
        // same exe that is already running.
        std::env::set_var(WEBVIEW2_ENV, &paths.webview2);
    }
    if std::env::var_os(DISABLE_CHAT_DISTILL_ENV).is_none() {
        // Meeting distill is feature-gated off in dev builds, but chat text is
        // real in every build: without this, every chat ended in a sandboxed
        // preview spends a live Claude call distilling fixture conversation.
        std::env::set_var(DISABLE_CHAT_DISTILL_ENV, "1");
    }
}

/// The directories Tauri would have used, computed the same way Tauri computes
/// them (`dirs::{data,config}_dir()` joined with the bundle identifier) because
/// there is no `AppHandle` this early.
fn real_app_dirs(identifier: &str) -> Result<RealAppDirs, String> {
    let data = dirs::data_dir()
        .ok_or_else(|| "could not resolve this machine's data directory".to_string())?;
    let config = dirs::config_dir()
        .ok_or_else(|| "could not resolve this machine's config directory".to_string())?;
    Ok(RealAppDirs {
        data: data.join(identifier),
        config: config.join(identifier),
    })
}

/// Reads an environment variable, treating empty as unset — the convention the
/// existing seams already follow.
fn non_empty(key: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(key).filter(|value| !value.is_empty())
}

fn refuse(message: &str) -> ! {
    eprintln!("kodabi: refusing to launch in sandbox mode.\n  {message}");
    std::process::exit(1);
}

/// Whether this process is running sandboxed.
pub(crate) fn active() -> bool {
    base().is_some()
}

fn base() -> Option<&'static PathBuf> {
    SANDBOX_BASE.get().and_then(|slot| slot.as_ref())
}

/// The app's config directory: `settings.toml`, `device.toml`, `_claude/`.
///
/// The third seam. Sandboxed, config lives in the sandbox base alongside the
/// vault — which is also the shape a real Windows install has, since
/// `app_config_dir()` and `app_data_dir()` resolve to the same folder there.
/// Unsandboxed, this is `app_config_dir()` unchanged.
pub(crate) fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(base) = base() {
        return Ok(base.clone());
    }
    app.path().app_config_dir().map_err(|err| {
        crate::user_errors::reported(
            "config_dir",
            err,
            "Kodabi couldn't find its settings folder. Restart the app; your notes on disk are \
             untouched.",
        )
    })
}

/// Where the models downloaded on first run live.
///
/// The fourth seam. Dot-prefixed because unsandboxed this sits *inside* the
/// default vault root (`app_data_dir()` is both), and a plain `models/` folder
/// there would enumerate as a project — the same reasoning that put the index in
/// `.index/`. Like the index it is derived, machine-local state: large, rebuilt
/// by re-downloading, and never something to sync.
///
/// Sandboxed runs get their own directory rather than borrowing the real one.
/// It is normally empty and that is correct: a debug build transcribes with the
/// mock engine and needs no models at all, so the alternative would be reaching
/// into the real app dir for nothing.
pub(crate) fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(base) = base() {
        return Ok(base.join(kodabi_core::sandbox::MODELS_SUBDIR));
    }
    app.path()
        .app_data_dir()
        .map(|dir| dir.join(kodabi_core::sandbox::MODELS_SUBDIR))
        .map_err(|err| {
            crate::user_errors::reported(
                "models_dir",
                err,
                "Kodabi couldn't find its data folder. Restart the app; your notes on disk are \
                 untouched.",
            )
        })
}
