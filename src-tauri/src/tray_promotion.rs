//! Puts Kodabi's tray mark on the visible taskbar rather than the hidden
//! overflow flyout.
//!
//! Windows adds every new notification icon to the overflow by default, and an
//! icon behind a chevron answers "am I being recorded?" only after a click —
//! which is exactly what the tray mark exists to avoid. Windows exposes no API
//! for this: `Shell_NotifyIcon` cannot promote an icon, and the documented
//! position is that only the user may. What Explorer actually reads is an
//! undocumented per-icon registry value, `IsPromoted` under
//! `HKCU\Control Panel\NotifyIconSettings\<id>`, and it takes effect live.
//!
//! So this promotes **once**, and only when the user has expressed no opinion.
//! Explorer writes `IsPromoted` explicitly in both directions (1 shown, 0
//! hidden), so an absent value means "never chosen" and a present one — either
//! value — means the choice is the user's and is left alone. A user who hides
//! the mark stays hidden through every later launch.
//!
//! Everything here is best-effort: it is an undocumented key on someone else's
//! taskbar, so a missing key, a changed shape or a denied write is logged and
//! shrugged off, never surfaced or retried into the app's startup path.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
use winreg::RegKey;

/// Explorer's per-icon settings, one subkey per notification icon.
const NOTIFY_ICON_SETTINGS: &str = r"Control Panel\NotifyIconSettings";
/// 1 shows the icon on the taskbar, 0 keeps it in the overflow flyout.
const IS_PROMOTED: &str = "IsPromoted";

/// How long to wait for Explorer to write our entry. It creates the subkey
/// when the icon is first added, which races `build_tray` returning.
const POLL_ATTEMPTS: u32 = 24;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// One `NotifyIconSettings` entry, reduced to what the decision needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IconEntry {
    /// The subkey name (an opaque hash of the icon's identity).
    key: String,
    /// The `ExecutablePath` value, absent on entries that don't carry one.
    executable_path: Option<String>,
    /// The `IsPromoted` value, absent until someone sets it.
    is_promoted: Option<u32>,
}

/// The entry to promote, if any.
///
/// Ours is the one whose `ExecutablePath` is this binary — compared
/// case-insensitively, since Windows paths are and Explorer records whatever
/// casing the process was launched with. An entry that already carries an
/// `IsPromoted` value is skipped whatever it says: 1 means the work is done,
/// 0 means the user deliberately hid the mark.
fn entry_to_promote<'a>(entries: &'a [IconEntry], executable: &Path) -> Option<&'a IconEntry> {
    let executable = executable.to_string_lossy().to_lowercase();
    entries.iter().find(|entry| {
        entry.is_promoted.is_none()
            && entry
                .executable_path
                .as_ref()
                .is_some_and(|path| path.to_lowercase() == executable)
    })
}

/// Reads every `NotifyIconSettings` entry.
fn read_entries() -> std::io::Result<Vec<IconEntry>> {
    let settings =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(NOTIFY_ICON_SETTINGS, KEY_READ)?;
    Ok(settings
        .enum_keys()
        .flatten()
        .filter_map(|key| {
            let entry = settings.open_subkey_with_flags(&key, KEY_READ).ok()?;
            Some(IconEntry {
                executable_path: entry.get_value("ExecutablePath").ok(),
                is_promoted: entry.get_value(IS_PROMOTED).ok(),
                key,
            })
        })
        .collect())
}

/// Sets `IsPromoted` on one entry. Explorer picks the change up live.
fn set_promoted(key: &str) -> std::io::Result<()> {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(format!(r"{NOTIFY_ICON_SETTINGS}\{key}"), KEY_SET_VALUE)?
        .set_value(IS_PROMOTED, &1u32)
}

/// One attempt. `Ok(true)` means there is nothing left to do — either the mark
/// was promoted, or the entry already carries the user's choice.
fn try_promote(executable: &Path) -> std::io::Result<bool> {
    let entries = read_entries()?;
    let Some(entry) = entry_to_promote(&entries, executable) else {
        // Either Explorer hasn't written our entry yet (poll again), or it has
        // and someone already set the value (nothing to do). Only the second
        // reading ends the polling.
        let ours_exists = entries.iter().any(|entry| {
            entry.executable_path.as_ref().is_some_and(|path| {
                path.to_lowercase() == executable.to_string_lossy().to_lowercase()
            })
        });
        return Ok(ours_exists);
    };
    set_promoted(&entry.key)?;
    Ok(true)
}

/// Promote the tray mark onto the taskbar in the background, if the user has
/// never said otherwise. Call once, after the tray icon has been built.
///
/// Runs off the startup path: the entry it needs is written by Explorer in
/// response to the icon being added, so it polls briefly rather than assuming
/// the entry is there the moment `build_tray` returns. Gives up quietly.
pub fn promote_in_background(executable: PathBuf) {
    thread::spawn(move || {
        for _ in 0..POLL_ATTEMPTS {
            match try_promote(&executable) {
                Ok(true) => return,
                Ok(false) => {}
                Err(err) => {
                    eprintln!("could not promote the tray icon to the taskbar: {err}");
                    return;
                }
            }
            thread::sleep(POLL_INTERVAL);
        }
        eprintln!("tray icon settings never appeared; leaving the mark in the overflow");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, executable_path: Option<&str>, is_promoted: Option<u32>) -> IconEntry {
        IconEntry {
            key: key.to_string(),
            executable_path: executable_path.map(str::to_string),
            is_promoted,
        }
    }

    #[test]
    fn promotes_our_own_entry_only() {
        let entries = [
            entry("aaa", Some(r"C:\Program Files\Other\other.exe"), None),
            entry("bbb", Some(r"C:\Program Files\Kodabi\kodabi.exe"), None),
            entry("ccc", None, None),
        ];
        let chosen = entry_to_promote(&entries, Path::new(r"C:\Program Files\Kodabi\kodabi.exe"));
        assert_eq!(chosen.map(|entry| entry.key.as_str()), Some("bbb"));
    }

    #[test]
    fn matches_the_executable_path_case_insensitively() {
        // Explorer records whatever casing the process was launched with, and
        // Windows paths don't distinguish it.
        let entries = [entry(
            "aaa",
            Some(r"C:\PROGRAM FILES\Kodabi\KODABI.EXE"),
            None,
        )];
        assert!(
            entry_to_promote(&entries, Path::new(r"c:\program files\kodabi\kodabi.exe")).is_some()
        );
    }

    #[test]
    fn never_overrides_a_choice_the_user_already_made() {
        let ours = r"C:\Program Files\Kodabi\kodabi.exe";
        // 0 is the user deliberately hiding the mark: leave it hidden, on this
        // launch and every later one.
        assert!(entry_to_promote(&[entry("aaa", Some(ours), Some(0))], Path::new(ours)).is_none());
        // 1 is already promoted, so there is nothing to write.
        assert!(entry_to_promote(&[entry("aaa", Some(ours), Some(1))], Path::new(ours)).is_none());
    }

    #[test]
    fn promotes_nothing_when_our_entry_is_missing() {
        // Explorer hasn't written the entry yet, or this build lives at a
        // different path than the one that has one. Promoting some other app's
        // icon would be strictly worse than doing nothing.
        let entries = [entry("aaa", Some(r"C:\Other\other.exe"), None)];
        assert!(
            entry_to_promote(&entries, Path::new(r"C:\Program Files\Kodabi\kodabi.exe")).is_none()
        );
    }
}
