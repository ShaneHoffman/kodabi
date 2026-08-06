//! Dev sandbox resolution: where an agent-driven launch is allowed to keep its
//! state, and when it must refuse to start.
//!
//! Kodabi keeps every piece of persistent state under a small number of
//! directories that are, on Windows, the *same* directory: the vault root
//! (`sessions/`, `chats/`, project note folders), the app config dir
//! (`settings.toml`, `device.toml`, `_claude/`), and the note index. That is
//! fine for a real install and dangerous for an automated one — a preview
//! session that captures audio, flips a setting, or lets a retention sweep run
//! would be doing it to the user's own notes.
//!
//! So `KODABI_SANDBOX` names a base directory that every one of those paths is
//! derived from, and this module is the derivation. It is deliberately *one*
//! switch rather than the pair of lower-level seams (`KODABI_KB_ROOT` and
//! `KODABI_INDEX_DB`) it drives: those two are only safe when set together —
//! moving the vault while the index stays behind makes the startup reconcile
//! converge the real index against a foreign vault and delete every row for the
//! notes it can no longer see — and a switch that cannot be half-set cannot be
//! half-set wrongly.
//!
//! **Refusal, not fallthrough.** Every failure here is a startup error rather
//! than a quiet fallback to the real directories. A sandbox that silently
//! resolved to real data would be worse than no sandbox at all, because the
//! caller believes it is isolated. See [`SandboxError`].
//!
//! Layout under the base, mirroring the Windows install shape so a sandboxed
//! run exercises the same code paths a real one does:
//!
//! - the base itself is both the vault root and the config dir,
//! - `.index/index.db` is the note index — dot-prefixed so vault enumeration
//!   skips it (a plain `index/` folder would show up as a project) and so it
//!   survives a re-seed of the fixture vault,
//! - `.webview2/` is the WebView2 user-data profile, kept out of the vault for
//!   the same reason.
//!
//! The index path must stay byte-identical to `indexDbFor()` in
//! `e2e/lib/vault.mjs`, which is what the fixture seeder uses — the two are
//! cross-referenced in both directions.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

/// The one switch. Unset or empty means "no sandbox": the app resolves every
/// path exactly as it always has.
pub const SANDBOX_ENV: &str = "KODABI_SANDBOX";

/// Values that select the default sandbox location rather than naming one.
const ENABLE_KEYWORDS: [&str; 2] = ["1", "true"];

/// Appended to the real app dir's final component to form the default sandbox
/// base (`com.kodabi.app` → `com.kodabi.app-dev`). A sibling rather than a
/// child, so the sandbox can never sit inside the directory it is isolating.
pub const DEV_SUFFIX: &str = "-dev";

/// Index location under the sandbox base. Mirrored by `indexDbFor()` in
/// `e2e/lib/vault.mjs` — change one and change the other.
pub const INDEX_SUBDIR: &str = ".index";
pub const INDEX_FILE: &str = "index.db";

/// WebView2 user-data profile under the sandbox base. Without a distinct
/// profile a sandboxed instance shares a browser process (and its localStorage)
/// with an already-running real instance, because Tauri leaves the variable
/// unset and WebView2 then derives the profile from the executable name.
pub const WEBVIEW2_SUBDIR: &str = ".webview2";

/// The real directories a non-sandboxed launch would have used — the ones the
/// sandbox exists to stay out of.
///
/// Two fields rather than one because Tauri derives them from different roots
/// (`dirs::data_dir()` and `dirs::config_dir()`, each joined with the bundle
/// identifier). On Windows they are the same folder, which is precisely why the
/// vault, the settings file and the index all share a directory there — but the
/// overlap check must not assume it.
#[derive(Debug, Clone)]
pub struct RealAppDirs {
    /// `app_data_dir()`: the default vault root and index home.
    pub data: PathBuf,
    /// `app_config_dir()`: `settings.toml`, `device.toml`, `_claude/`.
    pub config: PathBuf,
}

/// The raw environment a launch was given, snapshotted by the shell before any
/// Tauri machinery exists.
///
/// Each field holds only *non-empty* values: an empty environment variable is
/// treated as unset throughout Kodabi, and the shell applies that filter.
#[derive(Debug, Clone)]
pub struct SandboxEnv {
    /// `KODABI_SANDBOX` — a keyword (`1`/`true`) or an absolute path.
    pub switch: OsString,
    /// `KODABI_KB_ROOT`, if the caller also set it. Refused: see
    /// [`SandboxError::ExplicitOverride`].
    pub explicit_kb_root: Option<OsString>,
    /// `KODABI_INDEX_DB`, if the caller also set it. Refused likewise.
    pub explicit_index_db: Option<OsString>,
}

/// Every state location a sandboxed launch may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxPaths {
    /// Vault root *and* config dir, matching the Windows install shape.
    pub base: PathBuf,
    /// `<base>/.index/index.db`.
    pub index_db: PathBuf,
    /// `<base>/.webview2`.
    pub webview2: PathBuf,
}

/// Why a sandboxed launch must not proceed.
///
/// All three are refusals, never warnings. The messages are read by a developer
/// or an agent staring at a failed launch, so each says what to change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxError {
    /// The caller set the sandbox switch *and* one of the lower-level seams it
    /// drives. Honouring both is impossible and picking one silently is exactly
    /// the half-set hazard the switch exists to remove.
    #[error(
        "{variable} is set alongside {sandbox}, which derives the vault and index itself. \
         Unset {variable}, or drop {sandbox} to use it directly."
    )]
    ExplicitOverride {
        variable: &'static str,
        sandbox: &'static str,
    },
    /// A relative base would resolve against whatever the working directory
    /// happened to be — which for an app launched from a tray or a build tool
    /// is not something the caller chose.
    #[error("{sandbox} must be `1`, `true`, or an absolute path; got the relative path {base:?}")]
    RelativeBase {
        sandbox: &'static str,
        base: PathBuf,
    },
    /// The base is, contains, or sits inside the real app directory. Sandboxing
    /// into real data is the one outcome this whole module exists to prevent.
    #[error(
        "sandbox base {base:?} overlaps the real app directory {real:?}. \
         Refusing to touch real data in sandbox mode; choose a base outside it."
    )]
    OverlapsRealAppDir { base: PathBuf, real: PathBuf },
}

/// Resolves the sandbox layout, or refuses.
///
/// Pure: the caller supplies the environment it read and the real directories
/// it would otherwise have used, and gets back paths or a reason. Nothing here
/// touches the filesystem — the base usually does not exist yet, which is also
/// why the overlap check below compares paths lexically rather than
/// canonicalising them.
pub fn resolve(env: &SandboxEnv, real: &RealAppDirs) -> Result<SandboxPaths, SandboxError> {
    // Refuse the mixed form before doing anything else: if the caller set both,
    // no resolution of ours is the one they meant.
    if env.explicit_kb_root.is_some() {
        return Err(SandboxError::ExplicitOverride {
            variable: "KODABI_KB_ROOT",
            sandbox: SANDBOX_ENV,
        });
    }
    if env.explicit_index_db.is_some() {
        return Err(SandboxError::ExplicitOverride {
            variable: "KODABI_INDEX_DB",
            sandbox: SANDBOX_ENV,
        });
    }

    let base = if is_enable_keyword(&env.switch) {
        default_base(&real.data)
    } else {
        PathBuf::from(&env.switch)
    };

    if !base.is_absolute() {
        return Err(SandboxError::RelativeBase {
            sandbox: SANDBOX_ENV,
            base,
        });
    }
    for real_dir in [&real.data, &real.config] {
        if overlaps(&base, real_dir) {
            return Err(SandboxError::OverlapsRealAppDir {
                base,
                real: real_dir.clone(),
            });
        }
    }

    Ok(SandboxPaths {
        index_db: base.join(INDEX_SUBDIR).join(INDEX_FILE),
        webview2: base.join(WEBVIEW2_SUBDIR),
        base,
    })
}

/// Whether the switch selects the default location rather than naming one.
/// Case-insensitive, because `$env:KODABI_SANDBOX="True"` is the same intent.
fn is_enable_keyword(switch: &OsStr) -> bool {
    switch.to_str().is_some_and(|value| {
        ENABLE_KEYWORDS
            .iter()
            .any(|kw| value.eq_ignore_ascii_case(kw))
    })
}

/// The default sandbox base: the real app dir's sibling with [`DEV_SUFFIX`]
/// appended to its final component.
///
/// A sibling rather than a child so the two never nest, and derived from the
/// real dir rather than a hard-coded path so it follows the bundle identifier
/// without this module having to know it.
fn default_base(real_app_dir: &Path) -> PathBuf {
    let mut name = real_app_dir.file_name().unwrap_or_default().to_os_string();
    name.push(DEV_SUFFIX);
    real_app_dir.with_file_name(name)
}

/// Whether two paths occupy the same place in the tree: equal, or one an
/// ancestor of the other.
///
/// Compared component-wise rather than as strings, because a string prefix test
/// would call `com.kodabi.app-dev` a child of `com.kodabi.app` and refuse the
/// default sandbox. Components are matched ASCII-case-insensitively for
/// Windows; a non-UTF-8 component compares by its lossy form, which can only
/// ever make two components look *equal* — erring toward refusing a launch, the
/// safe direction. Symlinks and junctions are out of scope: nothing is
/// canonicalised, so a link that points into the real app dir is not detected.
///
/// A `..` anywhere in either path reports overlap outright. `Path::components`
/// drops `.` but keeps `..`, so a base that walks out of a sibling and back in
/// (`…\com.kodabi.app-dev\..\com.kodabi.app`) compares *unequal* component-wise
/// while the OS resolves it to the real app dir — the pairwise walk cannot see
/// that without canonicalising a path that usually does not exist yet. So a
/// parent-dir component is treated as unanalysable and refused, in the same
/// safe direction as the lossy comparison above.
fn overlaps(left: &Path, right: &Path) -> bool {
    let left: Vec<_> = left.components().collect();
    let right: Vec<_> = right.components().collect();
    if left
        .iter()
        .chain(right.iter())
        .any(|component| matches!(component, Component::ParentDir))
    {
        return true;
    }
    left.iter()
        .zip(right.iter())
        .all(|(a, b)| component_eq(a.as_os_str(), b.as_os_str()))
}

fn component_eq(left: &OsStr, right: &OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Windows-shaped throughout: these are the paths the app actually sees.
    const REAL: &str = r"C:\Users\dev\AppData\Roaming\com.kodabi.app";

    fn env(switch: &str) -> SandboxEnv {
        SandboxEnv {
            switch: OsString::from(switch),
            explicit_kb_root: None,
            explicit_index_db: None,
        }
    }

    /// The Windows shape: config and data are the same folder.
    fn real() -> RealAppDirs {
        RealAppDirs {
            data: PathBuf::from(REAL),
            config: PathBuf::from(REAL),
        }
    }

    #[test]
    fn keyword_one_resolves_to_the_dev_sibling() {
        let paths = resolve(&env("1"), &real()).expect("should resolve");
        assert_eq!(
            paths.base,
            PathBuf::from(r"C:\Users\dev\AppData\Roaming\com.kodabi.app-dev")
        );
    }

    #[test]
    fn keyword_true_is_case_insensitive() {
        for switch in ["true", "TRUE", "True"] {
            let paths = resolve(&env(switch), &real()).expect("should resolve");
            assert!(paths.base.ends_with("com.kodabi.app-dev"), "{switch}");
        }
    }

    #[test]
    fn absolute_path_becomes_the_base() {
        let paths = resolve(&env(r"D:\work\kodabi\.sandbox"), &real()).expect("resolves");
        assert_eq!(paths.base, PathBuf::from(r"D:\work\kodabi\.sandbox"));
    }

    #[test]
    fn derived_subpaths_match_the_documented_layout() {
        let paths = resolve(&env(r"D:\sandbox"), &real()).expect("should resolve");
        // Byte-identical to `indexDbFor()` in e2e/lib/vault.mjs.
        assert_eq!(paths.index_db, PathBuf::from(r"D:\sandbox\.index\index.db"));
        assert_eq!(paths.webview2, PathBuf::from(r"D:\sandbox\.webview2"));
    }

    #[test]
    fn a_trailing_separator_is_tolerated() {
        let paths = resolve(&env(r"D:\sandbox\"), &real()).expect("should resolve");
        assert_eq!(paths.index_db, PathBuf::from(r"D:\sandbox\.index\index.db"));
    }

    #[test]
    fn a_relative_base_is_refused() {
        let err = resolve(&env(r".sandbox"), &real()).expect_err("should refuse");
        assert!(matches!(err, SandboxError::RelativeBase { .. }), "{err:?}");
    }

    #[test]
    fn an_explicit_kb_root_is_refused_and_named() {
        let mut with_root = env("1");
        with_root.explicit_kb_root = Some(OsString::from(REAL));
        let err = resolve(&with_root, &real()).expect_err("should refuse");
        assert!(
            matches!(
                err,
                SandboxError::ExplicitOverride {
                    variable: "KODABI_KB_ROOT",
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("KODABI_KB_ROOT"));
    }

    #[test]
    fn an_explicit_index_db_is_refused_and_named() {
        let mut with_index = env("1");
        with_index.explicit_index_db = Some(OsString::from(r"C:\somewhere\index.db"));
        let err = resolve(&with_index, &real()).expect_err("should refuse");
        assert!(
            matches!(
                err,
                SandboxError::ExplicitOverride {
                    variable: "KODABI_INDEX_DB",
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn the_real_app_dir_itself_is_refused_whatever_its_case() {
        let err = resolve(
            &env(r"c:\users\dev\appdata\roaming\COM.KODABI.APP"),
            &real(),
        )
        .expect_err("should refuse");
        assert!(
            matches!(err, SandboxError::OverlapsRealAppDir { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_base_inside_the_real_app_dir_is_refused() {
        let err = resolve(
            &env(r"C:\Users\dev\AppData\Roaming\com.kodabi.app\sandbox"),
            &real(),
        )
        .expect_err("should refuse");
        assert!(
            matches!(err, SandboxError::OverlapsRealAppDir { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn a_base_containing_the_real_app_dir_is_refused() {
        let err = resolve(&env(r"C:\Users\dev\AppData"), &real()).expect_err("refuses");
        assert!(
            matches!(err, SandboxError::OverlapsRealAppDir { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn the_dev_sibling_is_not_mistaken_for_a_child() {
        // A string-prefix overlap test would refuse the default base, since
        // the real dir is a prefix of `com.kodabi.app-dev`. This is the case
        // that forces component-wise comparison.
        let paths = resolve(&env("1"), &real()).expect("should resolve");
        assert!(!overlaps(&paths.base, Path::new(REAL)));
    }

    #[test]
    fn a_config_dir_that_differs_from_the_data_dir_is_also_fenced() {
        // Where the two roots diverge, a base that clears the data dir can
        // still sit inside the config dir. Both are checked. (Shaped for the
        // host running the test, since `is_absolute` is platform-specific.)
        let split = RealAppDirs {
            data: PathBuf::from(REAL),
            config: PathBuf::from(r"C:\Users\dev\AppData\Local\com.kodabi.app"),
        };
        let err = resolve(
            &env(r"C:\Users\dev\AppData\Local\com.kodabi.app\sandbox"),
            &split,
        )
        .expect_err("should refuse");
        match err {
            SandboxError::OverlapsRealAppDir { real, .. } => {
                assert_eq!(real, split.config);
            }
            other => panic!("expected an overlap refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_base_that_climbs_back_into_the_real_app_dir_is_refused() {
        // `Path::components` normalises `.` away but keeps `..`, so a base that
        // walks out of a sibling and back in compares unequal component-wise
        // while the OS resolves it to the real app directory itself.
        let err = resolve(
            &env(r"C:\Users\dev\AppData\Roaming\com.kodabi.app-dev\..\com.kodabi.app"),
            &real(),
        )
        .expect_err("should refuse");
        assert!(
            matches!(err, SandboxError::OverlapsRealAppDir { .. }),
            "{err:?}"
        );
    }

    #[test]
    fn an_unrelated_sibling_is_allowed() {
        let paths = resolve(
            &env(r"C:\Users\dev\AppData\Roaming\kodabi-fixture"),
            &real(),
        )
        .expect("should resolve");
        assert!(paths.base.ends_with("kodabi-fixture"));
    }
}
