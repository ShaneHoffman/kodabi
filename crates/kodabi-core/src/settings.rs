//! Machine-local app settings: the recording-consent acknowledgement, the
//! raw-transcript retention policy, the capture-overlay visibility flags, and
//! the theme preference.
//!
//! Like [`crate::device`], this lives in local, per-machine app config rather
//! than inside the synced knowledge-base folder — consent and retention are a
//! property of *this* install's user, not of the notes, and syncing them would
//! silently carry one machine's choice to another. Same load/save discipline
//! as `device.rs`: [`load_or_create`] self-heals a corrupt file rather than
//! bricking startup, and [`save`] writes atomically (temp-then-rename).

use std::fs;
use std::io;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How long raw session transcripts are kept before pruning.
///
/// Internally tagged so it serializes identically to TOML (a `[retention]`
/// table with a `policy` key) and to JSON over IPC
/// (`{"policy":"keep_days","days":14}`) — one wire contract for both. Defaults
/// to [`RetentionPolicy::KeepAll`]: nothing is ever pruned until the user
/// explicitly picks a policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum RetentionPolicy {
    /// Keep every raw session forever. The default, and a no-op for pruning.
    #[default]
    KeepAll,
    /// Prune raw sessions older than `days`. `NonZeroU32` rejects `days = 0`
    /// during deserialization (both TOML and IPC), so a zero-day policy that
    /// would delete a session the instant it's written can't be represented.
    KeepDays { days: NonZeroU32 },
    /// Discard a raw session as soon as it has been successfully distilled
    /// into a note. Enforced forward-only at distill time, not by sweeping.
    DiscardAfterDistill,
}

/// Whether the always-on-top capture pill shows while a capture runs, split by
/// how the capture began.
///
/// The split exists because the two cases carry different amounts of surprise:
/// a capture the user started by pressing a key needs no reminder (default
/// `false`), while one an automatic detector started needs the strongest
/// possible one (default `true`) — it is the only thing standing between an
/// unattended start and an invisible recording.
///
/// A nested struct rather than two flat `Settings` fields so this asymmetry can
/// live in a hand-written [`Default`]; the derive on `Settings` would force both
/// to `false`. Auto-detection does not exist yet (see `docs/FOUNDING_DOC.md` §7):
/// `auto_captures` is modeled now so the setting predates the feature that reads
/// it, and so an existing install inherits the default-on choice when it lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OverlaySettings {
    /// Show the pill for captures the user starts (hotkey, tray, or IPC).
    pub manual_captures: bool,
    /// Show the pill for captures started by meeting auto-detection. Dormant
    /// until that feature ships; nothing passes
    /// [`crate::overlay::CaptureOrigin::AutoDetected`] today.
    pub auto_captures: bool,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            manual_captures: false,
            auto_captures: true,
        }
    }
}

/// Which theme the window wears.
///
/// `System` is the default and the honest one: the app has no opinion until the
/// user states one, and `design/tokens.css` already answers `prefers-color-scheme`
/// on its own. The other two are an override for a machine whose OS setting
/// doesn't match where the user actually works.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// How the app looks. A nested struct for one field because appearance is a
/// category rather than a flag, and the next thing that belongs here (a density
/// or a font-size preference) should not have to move this one to arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: Theme,
}

/// What the Settings mic test found: whether the microphone picks up the
/// speakers strongly enough to bleed into a capture's "you" channel — the
/// crosstalk `EchoCancelledSource` (`src-tauri/src/transcribe.rs`) cleans up
/// on every real meeting, made visible here before one starts. Mirrors
/// `kodabi_audio::MicTestOutcome` rather than reusing it: `kodabi-core` stays
/// free of `kodabi-audio`, a platform-IO crate, so `src-tauri` (the one layer
/// that depends on both) maps between them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum MicCheckOutcome {
    /// The mic didn't meaningfully hear the test tone — channels separate
    /// cleanly.
    Headphones,
    /// The mic clearly heard the tone played through the speakers: `echo_db`
    /// is how much louder it was than the pre-tone noise floor, `delay_ms`
    /// the acoustic delay before the mic picked it up.
    Speakers { echo_db: f32, delay_ms: f32 },
    /// The recording was silent throughout — too little signal to classify.
    MicSilent,
}

/// A stored mic-test result: what the last run found, and when. Advisory
/// only — the echo canceller runs on every capture regardless of whether
/// this exists or what it says; this is purely something to show the user.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MicCheckResult {
    #[serde(flatten)]
    pub outcome: MicCheckOutcome,
    pub measured_at: DateTime<Utc>,
}

/// The persisted app settings. `#[serde(default)]` makes every field optional
/// on load, so an older file missing a field (or a future file with an extra
/// one) still deserializes — forward/backward compatibility for a config the
/// user may carry across app versions.
///
/// `PartialEq` only, not `Eq`: `MicCheckOutcome::Speakers`'s `f32` fields
/// aren't `Eq`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// Whether the user has acknowledged the recording-consent nudge. Gates
    /// the first capture: until this is `true`, a capture toggle surfaces the
    /// nudge instead of recording.
    pub consent_acknowledged: bool,
    /// The raw-transcript retention policy.
    pub retention: RetentionPolicy,
    /// Capture-overlay visibility.
    pub overlay: OverlaySettings,
    /// Theme choice.
    pub appearance: AppearanceSettings,
    /// The most recent Settings mic-test result, if the user has ever run
    /// one. Last field deliberately: serde emits fields in declaration
    /// order, so appending leaves the existing JSON/TOML prefix
    /// byte-identical for anything mirroring the older shape.
    pub mic_check: Option<MicCheckResult>,
}

/// Loads the settings stored at `config_path`, writing defaults on first run.
/// Idempotent: subsequent calls return the same settings.
pub fn load_or_create(config_path: &Path) -> io::Result<Settings> {
    match read(config_path) {
        Ok(Some(settings)) => return Ok(settings),
        Ok(None) => {}
        // A corrupt config (bad TOML, an out-of-range value — e.g. from a
        // partial sync or manual edit) must not brick startup: discard it and
        // write fresh defaults. Note this resets `consent_acknowledged` to
        // `false` — the safe direction: re-ask for consent rather than
        // silently assume it from an unreadable file. Genuine I/O errors
        // (permissions, disk) still propagate.
        Err(err) if err.kind() == io::ErrorKind::InvalidData => {}
        Err(err) => return Err(err),
    }

    let settings = Settings::default();
    save(config_path, &settings)?;
    Ok(settings)
}

/// Persists `settings` to `config_path` via temp-file-then-rename, so a crash
/// or concurrent write can't leave a half-written, corrupt file.
pub fn save(config_path: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let serialized = toml::to_string_pretty(settings)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let tmp_path = unique_temp_path(config_path)?;
    fs::write(&tmp_path, serialized)?;
    fs::rename(&tmp_path, config_path)?;
    Ok(())
}

fn read(config_path: &Path) -> io::Result<Option<Settings>> {
    let contents = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let settings: Settings =
        toml::from_str(&contents).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(Some(settings))
}

/// Builds a sibling temp path unique to this process and attempt, e.g.
/// `settings.toml.4821-a3f19c02.tmp` — two concurrent writers must not share a
/// temp file, or one could rename a file the other is still writing.
fn unique_temp_path(config_path: &Path) -> io::Result<PathBuf> {
    let file_name = config_path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "config path has no file name")
    })?;
    let mut rand = [0u8; 4];
    getrandom::getrandom(&mut rand)
        .map_err(|err| io::Error::other(format!("OS RNG unavailable: {err}")))?;

    let mut tmp_name = file_name.to_os_string();
    tmp_name.push(format!(
        ".{}-{:08x}.tmp",
        std::process::id(),
        u32::from_le_bytes(rand)
    ));
    Ok(config_path.with_file_name(tmp_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keep_days(days: u32) -> RetentionPolicy {
        RetentionPolicy::KeepDays {
            days: NonZeroU32::new(days).unwrap(),
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let mut rand = [0u8; 4];
        getrandom::getrandom(&mut rand).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "kodabi-settings-{tag}-{}-{:08x}",
            std::process::id(),
            u32::from_le_bytes(rand)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_is_keep_all_and_unacknowledged() {
        let settings = Settings::default();
        assert!(!settings.consent_acknowledged);
        assert_eq!(settings.retention, RetentionPolicy::KeepAll);
    }

    #[test]
    fn overlay_defaults_are_off_for_manual_and_on_for_auto_detected() {
        // The asymmetry is the whole point of the hand-written Default: a
        // capture the user started needs no pill, an auto-detected one does.
        let overlay = Settings::default().overlay;
        assert!(!overlay.manual_captures);
        assert!(overlay.auto_captures);
    }

    #[test]
    fn load_or_create_writes_defaults_once_and_persists() {
        let dir = temp_dir("create");
        let path = dir.join("settings.toml");

        let first = load_or_create(&path).unwrap();
        assert!(path.exists());
        assert_eq!(first, Settings::default());

        // A second load returns the same settings without a rewrite changing them.
        let second = load_or_create(&path).unwrap();
        assert_eq!(first, second);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn each_policy_variant_round_trips_through_toml() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("settings.toml");

        for policy in [
            RetentionPolicy::KeepAll,
            keep_days(14),
            RetentionPolicy::DiscardAfterDistill,
        ] {
            let settings = Settings {
                consent_acknowledged: true,
                retention: policy,
                overlay: OverlaySettings::default(),
                appearance: AppearanceSettings::default(),
                mic_check: None,
            };
            save(&path, &settings).unwrap();
            assert_eq!(load_or_create(&path).unwrap(), settings);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn overlay_flags_round_trip_through_toml() {
        let dir = temp_dir("overlay-roundtrip");
        let path = dir.join("settings.toml");

        // Both flags flipped away from their defaults, so a round trip that
        // silently fell back to `Default` would fail rather than coincide.
        let settings = Settings {
            consent_acknowledged: true,
            retention: RetentionPolicy::KeepAll,
            overlay: OverlaySettings {
                manual_captures: true,
                auto_captures: false,
            },
            appearance: AppearanceSettings::default(),
            mic_check: None,
        };
        save(&path, &settings).unwrap();
        assert_eq!(load_or_create(&path).unwrap(), settings);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_days_toml_shape_is_stable() {
        // Locks the on-disk representation the internally-tagged enum produces,
        // so a serde attribute change that alters it fails a test.
        let toml = toml::to_string_pretty(&Settings {
            consent_acknowledged: true,
            retention: keep_days(14),
            overlay: OverlaySettings::default(),
            appearance: AppearanceSettings::default(),
            mic_check: None,
        })
        .unwrap();
        assert!(toml.contains("consent_acknowledged = true"), "{toml}");
        assert!(toml.contains("[retention]"), "{toml}");
        assert!(toml.contains(r#"policy = "keep_days""#), "{toml}");
        assert!(toml.contains("days = 14"), "{toml}");
        assert!(toml.contains("[overlay]"), "{toml}");
        assert!(toml.contains("manual_captures = false"), "{toml}");
        assert!(toml.contains("auto_captures = true"), "{toml}");
        assert!(toml.contains("[appearance]"), "{toml}");
        assert!(toml.contains(r#"theme = "system""#), "{toml}");
        // The scalar fields must precede the tables. TOML puts everything after
        // a table header inside it, so moving `consent_acknowledged` below
        // `[overlay]` would silently make it an overlay key.
        assert!(
            toml.find("consent_acknowledged").unwrap() < toml.find("[retention]").unwrap(),
            "{toml}"
        );
    }

    #[test]
    fn a_file_written_before_the_overlay_setting_existed_loads_with_defaults() {
        let dir = temp_dir("overlay-backcompat");
        let path = dir.join("settings.toml");
        // Verbatim shape of a pre-overlay install's settings.toml. It must load
        // without a migration, and must inherit auto_captures = true rather
        // than the `false` a derived Default would have produced.
        fs::write(
            &path,
            "consent_acknowledged = true\n\n[retention]\npolicy = \"keep_all\"\n",
        )
        .unwrap();

        let settings = load_or_create(&path).unwrap();
        assert!(settings.consent_acknowledged);
        assert_eq!(settings.overlay, OverlaySettings::default());
        assert!(settings.overlay.auto_captures);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_partial_overlay_table_fills_the_missing_flag_from_default() {
        let dir = temp_dir("overlay-partial");
        let path = dir.join("settings.toml");
        // `#[serde(default)]` on OverlaySettings itself (not just Settings) is
        // what makes a half-written table fill in rather than fail to parse.
        fs::write(
            &path,
            "consent_acknowledged = true\n\n[retention]\npolicy = \"keep_all\"\n\n[overlay]\nmanual_captures = true\n",
        )
        .unwrap();

        let settings = load_or_create(&path).unwrap();
        assert!(settings.overlay.manual_captures);
        assert!(settings.overlay.auto_captures);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_or_create_heals_a_corrupt_file_back_to_defaults() {
        let dir = temp_dir("corrupt");
        let path = dir.join("settings.toml");
        fs::write(&path, "}} not toml {{").unwrap();

        // Corrupt file self-heals to defaults — including consent back to false.
        let healed = load_or_create(&path).unwrap();
        assert_eq!(healed, Settings::default());
        assert!(!healed.consent_acknowledged);
        // The healed file round-trips on the next load.
        assert_eq!(load_or_create(&path).unwrap(), healed);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unknown_keys_are_tolerated_for_forward_compatibility() {
        let dir = temp_dir("forward");
        let path = dir.join("settings.toml");
        // A file written by a future version with an extra field must still load.
        fs::write(
            &path,
            "consent_acknowledged = true\nfuture_field = 7\n\n[retention]\npolicy = \"keep_all\"\n",
        )
        .unwrap();

        let settings = load_or_create(&path).unwrap();
        assert!(settings.consent_acknowledged);
        assert_eq!(settings.retention, RetentionPolicy::KeepAll);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn zero_days_is_rejected_and_heals_to_default() {
        let dir = temp_dir("zero-days");
        let path = dir.join("settings.toml");
        // `days = 0` can't deserialize into NonZeroU32 → treated as corrupt.
        fs::write(
            &path,
            "consent_acknowledged = true\n\n[retention]\npolicy = \"keep_days\"\ndays = 0\n",
        )
        .unwrap();

        let healed = load_or_create(&path).unwrap();
        assert_eq!(healed, Settings::default());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn json_wire_shape_is_stable_for_the_frontend_mirror() {
        // Locks the IPC JSON the TS `Settings`/`RetentionPolicy` types mirror.
        let json = serde_json::to_string(&Settings::default()).unwrap();
        assert_eq!(
            json,
            r#"{"consent_acknowledged":false,"retention":{"policy":"keep_all"},"overlay":{"manual_captures":false,"auto_captures":true},"appearance":{"theme":"system"},"mic_check":null}"#
        );

        let keep = serde_json::to_string(&Settings {
            consent_acknowledged: true,
            retention: keep_days(30),
            overlay: OverlaySettings {
                manual_captures: true,
                auto_captures: false,
            },
            appearance: AppearanceSettings { theme: Theme::Dark },
            mic_check: Some(MicCheckResult {
                outcome: MicCheckOutcome::Speakers {
                    echo_db: 12.5,
                    delay_ms: 85.0,
                },
                measured_at: DateTime::parse_from_rfc3339("2026-07-22T00:48:18Z")
                    .unwrap()
                    .with_timezone(&Utc),
            }),
        })
        .unwrap();
        assert_eq!(
            keep,
            r#"{"consent_acknowledged":true,"retention":{"policy":"keep_days","days":30},"overlay":{"manual_captures":true,"auto_captures":false},"appearance":{"theme":"dark"},"mic_check":{"outcome":"speakers","echo_db":12.5,"delay_ms":85.0,"measured_at":"2026-07-22T00:48:18Z"}}"#
        );
    }

    #[test]
    fn a_file_written_before_the_appearance_setting_existed_loads_with_defaults() {
        let dir = temp_dir("appearance-backcompat");
        let path = dir.join("settings.toml");
        // Verbatim shape of an install from before this field. It must load
        // without a migration and land on System, which is the only default
        // that cannot be wrong: it defers to the OS the way the app always did.
        fs::write(
            &path,
            "consent_acknowledged = true\n\n[retention]\npolicy = \"keep_all\"\n\n[overlay]\nmanual_captures = true\nauto_captures = false\n",
        )
        .unwrap();

        let settings = load_or_create(&path).unwrap();

        assert_eq!(settings.appearance.theme, Theme::System);
        // The fields that were already there are untouched by the addition.
        assert!(settings.consent_acknowledged);
        assert!(settings.overlay.manual_captures);
        assert!(!settings.overlay.auto_captures);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_theme_round_trips_through_toml() {
        let dir = temp_dir("theme-roundtrip");
        let path = dir.join("settings.toml");

        for theme in [Theme::System, Theme::Light, Theme::Dark] {
            let settings = Settings {
                appearance: AppearanceSettings { theme },
                ..Settings::default()
            };
            save(&path, &settings).unwrap();
            assert_eq!(load_or_create(&path).unwrap(), settings);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_mic_check_outcome_round_trips_through_toml() {
        let dir = temp_dir("mic-check-roundtrip");
        let path = dir.join("settings.toml");
        let measured_at = DateTime::parse_from_rfc3339("2026-07-22T00:48:18Z")
            .unwrap()
            .with_timezone(&Utc);

        for outcome in [
            MicCheckOutcome::Headphones,
            MicCheckOutcome::Speakers {
                echo_db: 12.5,
                delay_ms: 85.0,
            },
            MicCheckOutcome::MicSilent,
        ] {
            let settings = Settings {
                mic_check: Some(MicCheckResult {
                    outcome,
                    measured_at,
                }),
                ..Settings::default()
            };
            save(&path, &settings).unwrap();
            assert_eq!(load_or_create(&path).unwrap(), settings);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn absent_mic_check_round_trips_through_toml_as_none() {
        let dir = temp_dir("mic-check-absent-roundtrip");
        let path = dir.join("settings.toml");

        let settings = Settings::default();
        assert!(settings.mic_check.is_none());
        save(&path, &settings).unwrap();
        assert_eq!(load_or_create(&path).unwrap().mic_check, None);

        fs::remove_dir_all(&dir).unwrap();
    }
}
