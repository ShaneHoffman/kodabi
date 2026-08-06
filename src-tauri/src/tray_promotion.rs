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

/// The known folders Explorer abbreviates a path under, as
/// (environment variable naming the folder, its `FOLDERID` GUID). Only the two
/// that can hold an installed Kodabi: a per-machine install lands in
/// `Program Files`, and a 32-bit one would land in its x86 sibling.
const ABBREVIATED_FOLDERS: [(&str, &str); 2] = [
    ("ProgramFiles", "{6D809377-6AF0-444B-8957-A3773F02200E}"),
    (
        "ProgramFiles(x86)",
        "{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}",
    ),
];

/// The known-folder roots that exist on this machine, lowercased.
fn abbreviated_folders() -> Vec<(String, &'static str)> {
    ABBREVIATED_FOLDERS
        .iter()
        .filter_map(|(variable, id)| {
            std::env::var(variable)
                .ok()
                .map(|root| (root.to_lowercase(), *id))
        })
        .collect()
}

/// Every `ExecutablePath` spelling Explorer might have recorded for this
/// binary, lowercased.
///
/// Windows paths are case-insensitive and Explorer records whatever casing the
/// process was launched with, hence the lowercasing. Less obviously, Explorer
/// does not always store an absolute path: under a handful of known folders it
/// writes `{FOLDERID}\rest` instead. Which spelling Kodabi's own entry wears
/// therefore depends on where it was installed — the shipped installer is
/// per-user (Tauri's NSIS default, and NSIS is the only bundle target), so in
/// practice the path stays absolute. The known-folder forms are kept for the
/// installs that don't: a per-machine install under `Program Files`, or a
/// binary placed there by hand. Matching only the absolute form would leave
/// those sitting in the overflow forever.
fn executable_forms(executable: &Path, folders: &[(String, &str)]) -> Vec<String> {
    let absolute = executable.to_string_lossy().to_lowercase();
    let mut forms = vec![absolute.clone()];
    for (root, id) in folders {
        // The trailing separator matters: without it `C:\Program Files` also
        // prefixes `C:\Program Files (x86)\…` and would abbreviate it wrongly.
        let prefix = format!(r"{}\", root.trim_end_matches('\\'));
        if let Some(rest) = absolute.strip_prefix(&prefix) {
            forms.push(format!(r"{}\{rest}", id.to_lowercase()));
        }
    }
    forms
}

/// Whether an entry is this binary's.
fn is_ours(entry: &IconEntry, forms: &[String]) -> bool {
    entry
        .executable_path
        .as_ref()
        .is_some_and(|path| forms.contains(&path.to_lowercase()))
}

/// The entry to promote, if any: ours, in any of the spellings
/// [`executable_forms`] accepts. An entry that already carries an `IsPromoted`
/// value is skipped whatever it says: 1 means the work is done, 0 means the
/// user deliberately hid the mark.
fn entry_to_promote<'a>(entries: &'a [IconEntry], forms: &[String]) -> Option<&'a IconEntry> {
    entries
        .iter()
        .find(|entry| entry.is_promoted.is_none() && is_ours(entry, forms))
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
    let forms = executable_forms(executable, &abbreviated_folders());
    let entries = read_entries()?;
    let Some(entry) = entry_to_promote(&entries, &forms) else {
        // Either Explorer hasn't written our entry yet (poll again), or it has
        // and someone already set the value (nothing to do). Only the second
        // reading ends the polling.
        let ours_exists = entries.iter().any(|entry| is_ours(entry, &forms));
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

    /// The known folders, fixed rather than read from this machine's
    /// environment, so the tests describe the same Windows everywhere.
    fn folders() -> Vec<(String, &'static str)> {
        vec![
            (
                r"c:\program files".to_string(),
                "{6D809377-6AF0-444B-8957-A3773F02200E}",
            ),
            (
                r"c:\program files (x86)".to_string(),
                "{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}",
            ),
        ]
    }

    fn forms(executable: &str) -> Vec<String> {
        executable_forms(Path::new(executable), &folders())
    }

    #[test]
    fn promotes_our_own_entry_only() {
        let entries = [
            entry("aaa", Some(r"C:\Program Files\Other\other.exe"), None),
            entry("bbb", Some(r"C:\Program Files\Kodabi\kodabi.exe"), None),
            entry("ccc", None, None),
        ];
        let chosen = entry_to_promote(&entries, &forms(r"C:\Program Files\Kodabi\kodabi.exe"));
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
            entry_to_promote(&entries, &forms(r"c:\program files\kodabi\kodabi.exe")).is_some()
        );
    }

    #[test]
    fn matches_the_known_folder_form_explorer_writes_for_program_files() {
        // Explorer stores a path under Program Files abbreviated to its
        // FOLDERID, not absolute — so a per-machine install would never
        // recognise its own entry if we only compared the path `current_exe`
        // reports.
        let entries = [entry(
            "aaa",
            Some(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Kodabi\Kodabi.exe"),
            None,
        )];
        assert!(
            entry_to_promote(&entries, &forms(r"C:\Program Files\Kodabi\Kodabi.exe")).is_some()
        );

        let x86 = [entry(
            "bbb",
            Some(r"{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}\Kodabi\Kodabi.exe"),
            None,
        )];
        assert!(
            entry_to_promote(&x86, &forms(r"C:\Program Files (x86)\Kodabi\Kodabi.exe")).is_some()
        );
    }

    #[test]
    fn does_not_abbreviate_program_files_x86_as_program_files() {
        // `C:\Program Files` prefixes `C:\Program Files (x86)` as a string, so
        // a separator-blind match would claim the x86 install wears the x64
        // FOLDERID and promote nothing (or worse, someone else's entry).
        let x86 = forms(r"C:\Program Files (x86)\Kodabi\Kodabi.exe");
        assert!(!x86
            .iter()
            .any(|form| form.starts_with("{6d809377-6af0-444b-8957-a3773f02200e}")));
    }

    #[test]
    fn never_overrides_a_choice_the_user_already_made() {
        let ours = r"C:\Program Files\Kodabi\kodabi.exe";
        // 0 is the user deliberately hiding the mark: leave it hidden, on this
        // launch and every later one.
        assert!(entry_to_promote(&[entry("aaa", Some(ours), Some(0))], &forms(ours)).is_none());
        // 1 is already promoted, so there is nothing to write.
        assert!(entry_to_promote(&[entry("aaa", Some(ours), Some(1))], &forms(ours)).is_none());
    }

    #[test]
    fn promotes_nothing_when_our_entry_is_missing() {
        // Explorer hasn't written the entry yet, or this build lives at a
        // different path than the one that has one. Promoting some other app's
        // icon would be strictly worse than doing nothing.
        let entries = [entry("aaa", Some(r"C:\Other\other.exe"), None)];
        assert!(
            entry_to_promote(&entries, &forms(r"C:\Program Files\Kodabi\kodabi.exe")).is_none()
        );
    }
}
