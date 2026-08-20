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

use kodabi_core::settings::{
    self, AppearanceSettings, CategorySettings, IdentitySettings, LedgerSettings, OverlaySettings,
    RetentionPolicy, Settings,
};
use tauri::{AppHandle, Emitter, Manager, State};

/// Event emitted after settings change over IPC, carrying the new [`Settings`].
/// Lets a view already mounted when the change lands (e.g. the Settings view
/// open while the consent nudge acknowledges) refresh without a reload — the
/// mutating caller still gets the echoed result directly, so this is only for
/// the *other* listeners.
pub const SETTINGS_CHANGED_EVENT: &str = "settings:changed";

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

    /// A copy of the current settings, so the lock is never held across a
    /// caller's work. A clone rather than a `Copy`: `IdentitySettings` carries
    /// a `Vec` of alias spellings. Every other field is still `Copy`, so the
    /// usual `snapshot().ledger` / `snapshot().categories` read costs nothing
    /// beyond the one short-lived allocation.
    pub fn snapshot(&self) -> Settings {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Applies `mutate` to the settings, persists the result, and returns it.
    /// The disk write happens under the lock so concurrent updates can't
    /// interleave into a torn file. `pub(crate)` so `audio_cmds::run_mic_test`
    /// can persist its result through the same write-through path other
    /// settings mutations use.
    ///
    /// `failed` is the caller's sentence for a write that didn't land. It has to
    /// come from the call site: `settings::save` returns a bare `io::Error`
    /// ("Access is denied. (os error 5)") that names neither the setting nor
    /// what still applies, and the in-memory value is left unchanged on failure,
    /// which is the reassurance each caller words for its own row.
    pub(crate) fn update(
        &self,
        failed: &str,
        mutate: impl FnOnce(&mut Settings),
    ) -> Result<Settings, String> {
        let mut guard = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut next = guard.clone();
        mutate(&mut next);
        settings::save(&self.path, &next)
            .map_err(|err| crate::user_errors::reported("settings", err, failed))?;
        *guard = next.clone();
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
    let settings = state.update(
        "Couldn't save the retention policy. The previous policy still applies; try again.",
        |s| s.retention = policy,
    )?;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    crate::retention::spawn_prune(&app);
    Ok(settings)
}

/// Sets the capture-overlay visibility flags and brings the pill in line with
/// them immediately, so flipping the toggle during a running capture shows or
/// hides it right then rather than at the next capture. That includes a capture
/// whose pill the user already dismissed — see
/// [`crate::overlay::apply_settings_change`].
#[tauri::command]
pub fn set_capture_overlay(
    app: AppHandle,
    state: State<'_, SettingsState>,
    overlay: OverlaySettings,
) -> Result<Settings, String> {
    let settings = state.update(
        "Couldn't save the capture pill setting. The previous setting still applies; try again.",
        |s| s.overlay = overlay,
    )?;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    crate::overlay::apply_settings_change(&app);
    Ok(settings)
}

/// Sets the theme preference.
///
/// No side effect beyond the event, unlike its neighbours: the frontend applies
/// the theme itself (`src/theme.ts`), and the event is what carries the change
/// to the quick-capture and overlay windows, which are separate webviews with
/// no other way to hear about it.
#[tauri::command]
pub fn set_appearance(
    app: AppHandle,
    state: State<'_, SettingsState>,
    appearance: AppearanceSettings,
) -> Result<Settings, String> {
    let settings = state.update(
        "Couldn't save the theme. The previous theme still applies; try again.",
        |s| s.appearance = appearance,
    )?;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    Ok(settings)
}

/// Sets the commitment-ledger tuning: the aging and stale day thresholds, and
/// how sure a conversation has to be before a completion claim closes an entry
/// on its own.
///
/// Emits the ledger event as well as the settings one, because the thresholds
/// are read when the commitments list is assembled: without it the Commitments
/// view would keep rendering the tiers it was given until something else
/// happened to refetch.
#[tauri::command]
pub fn set_ledger_tuning(
    app: AppHandle,
    state: State<'_, SettingsState>,
    ledger: LedgerSettings,
) -> Result<Settings, String> {
    let settings = state.update(
        "Couldn't save the commitment settings. The previous values still apply; try again.",
        |s| s.ledger = ledger,
    )?;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    let _ = app.emit(crate::events::LEDGER_CHANGED_EVENT, ());
    Ok(settings)
}

/// Sets the per-genre enrollment defaults: which kinds of meeting feed the
/// commitment ledger in full, and which contribute only what was asked of you
/// directly.
///
/// **Prospective, deliberately.** Unlike recategorizing one meeting, changing a
/// genre's default does not re-evaluate the entries existing meetings already
/// produced: it decides what happens the next time each note syncs. A setting
/// that silently rearranged the working set of every all-hands in the vault, at
/// whatever later moment each note happened to be re-read, would be a much
/// larger action than the control looks like. Recategorizing a meeting, or
/// flipping its own tracking switch, is the per-meeting re-evaluation.
///
/// Hence no `ledger:changed` either: nothing in the ledger has changed yet.
#[tauri::command]
pub fn set_category_prefs(
    app: AppHandle,
    state: State<'_, SettingsState>,
    categories: CategorySettings,
) -> Result<Settings, String> {
    let settings = state.update(
        "Couldn't save the meeting-kind settings. The previous values still apply; try again.",
        |s| s.categories = categories,
    )?;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    Ok(settings)
}

/// Sets who the user is: the name they go by, plus the other spellings meetings
/// use for them.
///
/// **Retrospective, unlike `set_category_prefs`.** Learning a name is not a
/// preference about what happens next; it is the answer to a question the
/// ledger has already been guessing at, and every commitment filed under
/// Waiting-on-them because the app did not know the user's name is wrong right
/// now. So the save is followed by a sweep
/// (`ledger::Ledger::retro_resolve_owners`), which moves untouched open entries
/// into Mine and never the other way.
///
/// The sweep is best-effort by design: the name is saved either way, and a
/// failure here costs re-filing that the next claim or the next sync redoes.
/// What the user asked for - "this is my name" - has landed regardless.
#[tauri::command]
pub fn set_identity(
    app: AppHandle,
    state: State<'_, SettingsState>,
    identity: IdentitySettings,
) -> Result<Settings, String> {
    // Normalized at the boundary, so what is stored is what will be matched.
    let identity = identity.normalized();
    let settings = state.update(
        "Couldn't save your name. The previous value still applies; try again.",
        |s| s.identity = identity,
    )?;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    resolve_owners_in_background(&app, settings.identity.owner_identity());
    Ok(settings)
}

/// Re-files the commitments an identity change re-resolves, off the IPC thread.
///
/// Spawned rather than awaited because the caller is a synchronous command and
/// the sweep talks to the ledger worker: the settings write is what the user is
/// waiting on, and the re-filing announces itself through `ledger:changed`
/// whenever it lands.
pub(crate) fn resolve_owners_in_background(
    app: &AppHandle,
    identity: kodabi_core::ledger::OwnerIdentity,
) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let client = app.state::<crate::ledger_state::LedgerState>().client();
        match client.retro_resolve_owners(identity) {
            Ok(outcome) if outcome.claimed.is_empty() => {}
            Ok(outcome) => {
                let _ = app.emit(crate::events::LEDGER_CHANGED_EVENT, ());
                eprintln!(
                    "identity: re-filed {} commitment(s) as yours",
                    outcome.claimed.len()
                );
            }
            Err(err) => {
                eprintln!("identity: couldn't re-check existing commitments: {err:?}");
            }
        }
    });
}

/// Records that the user acknowledged the recording-consent nudge and stores
/// the retention policy they chose in the same write. After this, the capture
/// gate lets recording proceed.
///
/// `display_name` is the first-run seed for the ledger's mine/theirs split, and
/// is optional in both senses: an `Option` parameter, so a payload that omits
/// the key deserializes to `None` and every existing caller is unaffected, and
/// the user may leave the field blank. This is the one gate every
/// install passes before its first capture, which is the last moment the answer
/// is still ahead of the first meeting rather than behind it. A blank leaves
/// the setting untouched rather than writing an empty name over one the user
/// already gave.
#[tauri::command]
pub fn acknowledge_consent(
    app: AppHandle,
    state: State<'_, SettingsState>,
    retention: RetentionPolicy,
    display_name: Option<String>,
) -> Result<Settings, String> {
    let display_name = display_name.unwrap_or_default().trim().to_string();
    let settings = state.update(
        "Couldn't save your choice, so recording stays off. Try again.",
        |s| {
            s.consent_acknowledged = true;
            s.retention = retention;
            if !display_name.is_empty() {
                s.identity.display_name = display_name.clone();
            }
        },
    )?;
    let _ = app.emit(SETTINGS_CHANGED_EVENT, settings.clone());
    // The chosen policy may prune immediately (e.g. KeepDays over pre-existing
    // sessions), same as an explicit policy change.
    crate::retention::spawn_prune(&app);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the tests name a concrete theme; the command signature takes the
    // whole AppearanceSettings, so this would be unused in a normal build.
    use kodabi_core::settings::Theme;

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
    fn a_meeting_kind_preference_persists_and_leaves_the_others_alone() {
        let (state, path) = temp_settings_state("categories");
        assert!(state
            .snapshot()
            .categories
            .all_hands
            .enrollment_default
            .is_none());

        let mut next = state.snapshot().categories;
        next.client = kodabi_core::settings::CategoryPrefs {
            enrollment_default: Some(kodabi_core::ledger::EnrollmentMode::ContextOnly),
        };
        let updated = state.update("unused", |s| s.categories = next).unwrap();

        assert_eq!(
            updated.categories.client.enrollment_default,
            Some(kodabi_core::ledger::EnrollmentMode::ContextOnly)
        );
        // Untouched kinds keep inheriting rather than being pinned to whatever
        // they resolved to when the write happened.
        assert!(updated.categories.all_hands.enrollment_default.is_none());

        let reread = settings::load_or_create(&path).unwrap();
        assert_eq!(
            reread.categories.client.enrollment_default,
            Some(kodabi_core::ledger::EnrollmentMode::ContextOnly)
        );
    }

    #[test]
    fn update_persists_and_reflects_in_snapshot() {
        let (state, path) = temp_settings_state("update");
        assert!(!state.snapshot().consent_acknowledged);

        let updated = state
            .update("unused", |s| s.consent_acknowledged = true)
            .unwrap();
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

    #[test]
    fn overlay_flags_persist_through_update() {
        let (state, path) = temp_settings_state("overlay");
        assert_eq!(state.snapshot().overlay, OverlaySettings::default());

        let updated = state
            .update("unused", |s| {
                s.overlay = OverlaySettings {
                    manual_captures: true,
                    auto_captures: false,
                }
            })
            .unwrap();
        assert!(updated.overlay.manual_captures);
        assert!(!updated.overlay.auto_captures);

        // Persisted, so a restart keeps the user's choice.
        let reloaded = settings::load_or_create(&path).unwrap();
        assert!(reloaded.overlay.manual_captures);
        assert!(!reloaded.overlay.auto_captures);

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn appearance_persists_through_update() {
        let (state, path) = temp_settings_state("appearance");
        assert_eq!(state.snapshot().appearance.theme, Theme::System);

        let updated = state
            .update("unused", |s| {
                s.appearance = AppearanceSettings { theme: Theme::Dark }
            })
            .unwrap();
        assert_eq!(updated.appearance.theme, Theme::Dark);

        // Persisted: the window must not open in the wrong theme after a restart.
        let reloaded = settings::load_or_create(&path).unwrap();
        assert_eq!(reloaded.appearance.theme, Theme::Dark);

        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
