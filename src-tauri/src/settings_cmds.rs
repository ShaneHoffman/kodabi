//! Thin Tauri command wrappers over `kodabi_core::settings`. The core module
//! owns the on-disk format and load/save discipline; this module owns the
//! managed [`SettingsState`] (a write-through cache over the settings file) and
//! the IPC surface the frontend's consent nudge and Settings view drive.
//!
//! The core [`Settings`]/[`RetentionPolicy`] types are reused directly as the
//! IPC DTOs — they already derive `Serialize`/`Deserialize`, so no mirror
//! structs are needed (the thin-wrapper rule).

use std::path::PathBuf;
use std::sync::Mutex;

use kodabi_core::settings::{self, RetentionPolicy, Settings};
use tauri::{AppHandle, State};

/// The app's settings, cached in memory over the on-disk `settings.toml`.
///
/// Write-through: mutations take the lock, persist to disk, and only then
/// release — so an in-memory read is always consistent with the file, and the
/// consent check on the capture hot path never has to touch the disk (a
/// transient read error there would risk a wrong gate decision).
pub struct SettingsState {
    path: PathBuf,
    current: Mutex<Settings>,
}

impl SettingsState {
    /// Wraps the already-loaded settings and the path they persist to. The
    /// initial load happens once at startup via `settings::load_or_create`.
    pub fn new(path: PathBuf, initial: Settings) -> Self {
        Self {
            path,
            current: Mutex::new(initial),
        }
    }

    /// A copy of the current settings. `Settings` is `Copy`, so this never
    /// holds the lock across a caller's work.
    pub fn snapshot(&self) -> Settings {
        *self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Applies `mutate` to the settings, persists the result, and returns it.
    /// The disk write happens under the lock so concurrent updates can't
    /// interleave into a torn file.
    fn update(&self, mutate: impl FnOnce(&mut Settings)) -> Result<Settings, String> {
        let mut guard = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = *guard;
        mutate(&mut next);
        settings::save(&self.path, &next).map_err(|err| err.to_string())?;
        *guard = next;
        Ok(next)
    }
}

/// Returns the current settings — the frontend seeds its Settings view and
/// consent state from this on mount.
#[tauri::command]
pub fn get_settings(state: State<'_, SettingsState>) -> Settings {
    state.snapshot()
}

/// Sets the retention policy, persists it, and kicks off an immediate prune so
/// a newly chosen `KeepDays` takes effect at once rather than at the next
/// scheduled sweep.
#[tauri::command]
pub fn set_retention_policy(
    app: AppHandle,
    state: State<'_, SettingsState>,
    policy: RetentionPolicy,
) -> Result<Settings, String> {
    let settings = state.update(|s| s.retention = policy)?;
    crate::retention::spawn_prune(&app);
    Ok(settings)
}

/// Records that the user acknowledged the recording-consent nudge and stores
/// the retention policy they chose in the same write. After this, the capture
/// gate lets recording proceed.
#[tauri::command]
pub fn acknowledge_consent(
    app: AppHandle,
    state: State<'_, SettingsState>,
    retention: RetentionPolicy,
) -> Result<Settings, String> {
    let settings = state.update(|s| {
        s.consent_acknowledged = true;
        s.retention = retention;
    })?;
    // The chosen policy may prune immediately (e.g. KeepDays over pre-existing
    // sessions), same as an explicit policy change.
    crate::retention::spawn_prune(&app);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_settings_state(tag: &str) -> (SettingsState, PathBuf) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kodabi-settings-cmds-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        let initial = settings::load_or_create(&path).unwrap();
        (SettingsState::new(path.clone(), initial), path)
    }

    #[test]
    fn update_persists_and_reflects_in_snapshot() {
        let (state, path) = temp_settings_state("update");
        assert!(!state.snapshot().consent_acknowledged);

        let updated = state.update(|s| s.consent_acknowledged = true).unwrap();
        assert!(updated.consent_acknowledged);
        assert!(state.snapshot().consent_acknowledged);

        // Persisted: a fresh load from disk sees the change.
        assert!(
            settings::load_or_create(&path)
                .unwrap()
                .consent_acknowledged
        );

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
