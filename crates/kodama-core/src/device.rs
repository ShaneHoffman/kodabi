//! Stable per-device identity, generated once and persisted locally.
//!
//! The ID distinguishes devices in session filenames (see [`crate::naming`]).
//! It deliberately lives in local, per-machine app config rather than inside
//! the synced knowledge-base folder — if it synced, two devices would share
//! the same value and defeat the collision-avoidance the filename scheme
//! relies on (see `docs/FILENAME_SCHEME.md`).

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

const ID_LEN: usize = 8;
const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// An 8-character lowercase base36 device identifier, e.g. `k4m2xp7q`.
///
/// Only needs to distinguish a user's own handful of devices, not be
/// globally unique — 36^8 (≈2.8e12) makes collision negligible at that
/// scale.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceId(String);

impl DeviceId {
    /// Borrows the underlying 8-character base36 string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validates and wraps an existing string as a `DeviceId`.
    pub fn parse(s: &str) -> Result<Self, InvalidDeviceId> {
        if s.len() == ID_LEN && s.bytes().all(|b| ALPHABET.contains(&b)) {
            Ok(Self(s.to_string()))
        } else {
            Err(InvalidDeviceId)
        }
    }

    /// Generates a fresh random device ID from OS entropy.
    pub fn generate() -> Self {
        let mut raw = [0u8; ID_LEN];
        getrandom::getrandom(&mut raw).expect("OS RNG unavailable");
        let id: String = raw
            .iter()
            .map(|byte| ALPHABET[(*byte as usize) % ALPHABET.len()] as char)
            .collect();
        Self(id)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A string failed to parse as a valid [`DeviceId`].
#[derive(Debug)]
pub struct InvalidDeviceId;

impl fmt::Display for InvalidDeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "device id must be exactly {ID_LEN} lowercase base36 characters"
        )
    }
}

impl std::error::Error for InvalidDeviceId {}

#[derive(Serialize, Deserialize)]
struct DeviceConfig {
    device_id: String,
}

/// Loads the device ID stored at `config_path`, generating and persisting a
/// new one on first run. Idempotent: subsequent calls return the same ID.
pub fn load_or_create(config_path: &Path) -> io::Result<DeviceId> {
    if let Some(existing) = read(config_path)? {
        return Ok(existing);
    }

    let id = DeviceId::generate();
    write_atomic(config_path, &id)?;
    Ok(id)
}

fn read(config_path: &Path) -> io::Result<Option<DeviceId>> {
    let contents = match fs::read_to_string(config_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let config: DeviceConfig =
        toml::from_str(&contents).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let id = DeviceId::parse(&config.device_id)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(Some(id))
}

/// Writes the device config via temp-file-then-rename so a first-run race
/// (two launches at once) can't leave a half-written, corrupt file.
fn write_atomic(config_path: &Path, id: &DeviceId) -> io::Result<()> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let config = DeviceConfig {
        device_id: id.as_str().to_string(),
    };
    let serialized = toml::to_string_pretty(&config)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

    let mut tmp_name = OsString::from(config_path.as_os_str());
    tmp_name.push(".tmp");
    let tmp_path = config_path.with_file_name(tmp_name);

    fs::write(&tmp_path, serialized)?;
    fs::rename(&tmp_path, config_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_id_is_eight_lowercase_base36_chars() {
        let id = DeviceId::generate();
        assert_eq!(id.as_str().len(), ID_LEN);
        assert!(id.as_str().bytes().all(|b| ALPHABET.contains(&b)));
    }

    #[test]
    fn parse_rejects_wrong_length_and_uppercase() {
        assert!(DeviceId::parse("shortid").is_err());
        assert!(DeviceId::parse("toolongdeviceid").is_err());
        assert!(DeviceId::parse("K4M2XP7Q").is_err());
        assert!(DeviceId::parse("k4m2xp7q").is_ok());
    }

    #[test]
    fn load_or_create_generates_once_and_persists() {
        let dir = std::env::temp_dir().join(format!("kodama-device-test-{}", DeviceId::generate()));
        fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("device.toml");

        let first = load_or_create(&config_path).unwrap();
        assert!(config_path.exists());

        let second = load_or_create(&config_path).unwrap();
        assert_eq!(first, second);

        let contents = fs::read_to_string(&config_path).unwrap();
        let parsed: DeviceConfig = toml::from_str(&contents).unwrap();
        assert!(DeviceId::parse(&parsed.device_id).is_ok());

        fs::remove_dir_all(&dir).unwrap();
    }
}
