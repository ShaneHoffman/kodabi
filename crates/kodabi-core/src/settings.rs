//! Machine-local app settings: the recording-consent acknowledgement and the
//! raw-transcript retention policy.
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

/// The persisted app settings. `#[serde(default)]` makes every field optional
/// on load, so an older file missing a field (or a future file with an extra
/// one) still deserializes — forward/backward compatibility for a config the
/// user may carry across app versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    /// Whether the user has acknowledged the recording-consent nudge. Gates
    /// the first capture: until this is `true`, a capture toggle surfaces the
    /// nudge instead of recording.
    pub consent_acknowledged: bool,
    /// The raw-transcript retention policy.
    pub retention: RetentionPolicy,
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
            };
            save(&path, &settings).unwrap();
            assert_eq!(load_or_create(&path).unwrap(), settings);
        }

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_days_toml_shape_is_stable() {
        // Locks the on-disk representation the internally-tagged enum produces,
        // so a serde attribute change that alters it fails a test.
        let toml = toml::to_string_pretty(&Settings {
            consent_acknowledged: true,
            retention: keep_days(14),
        })
        .unwrap();
        assert!(toml.contains("consent_acknowledged = true"), "{toml}");
        assert!(toml.contains("[retention]"), "{toml}");
        assert!(toml.contains(r#"policy = "keep_days""#), "{toml}");
        assert!(toml.contains("days = 14"), "{toml}");
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
            r#"{"consent_acknowledged":false,"retention":{"policy":"keep_all"}}"#
        );

        let keep = serde_json::to_string(&Settings {
            consent_acknowledged: true,
            retention: keep_days(30),
        })
        .unwrap();
        assert_eq!(
            keep,
            r#"{"consent_acknowledged":true,"retention":{"policy":"keep_days","days":30}}"#
        );
    }
}
