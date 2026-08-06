//! Where the app looks for its models, and who is allowed to answer.
//!
//! Two sources, in order:
//!
//! 1. **The environment**, wholesale per model set. Set all five `PARAKEET_*`
//!    variables and that is where the STT engine loads from; set
//!    `KODABI_EMBED_MODEL_DIR` and that is where embeddings load from. This is
//!    the developer and CI path (`.github/workflows/ci.yml` sets exactly those
//!    five for the real-model tests), and it wins outright: a set pointed
//!    elsewhere is never downloaded and never reported missing.
//! 2. **The models directory** ([`sandbox::models_dir`]), populated on first run
//!    by [`models_cmds`](crate::models_cmds). This is what a downloaded install
//!    uses.
//!
//! Wholesale rather than per-variable, because a half-set STT override is not a
//! configuration anyone meant: four real paths and one empty one produces a
//! `ModelLoad` failure naming a blank file, and silently filling the gap from
//! the models directory would mix two model versions inside one engine. A
//! partial override is therefore ignored with a warning, which is the same
//! "refusal, not fallthrough" posture the sandbox module takes.
//!
//! Layout is not restated here: the paths are derived from the compiled-in
//! manifest, so the manifest is the single source of truth for what is where.

use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::sandbox;

/// Manifest set ids. These are the strings in
/// `crates/kodabi-core/src/models/manifest.json`.
pub(crate) const PARAKEET_SET_ID: &str = "parakeet-tdt-0.6b-v2-int8";
pub(crate) const SILERO_SET_ID: &str = "silero-vad";
pub(crate) const EMBED_SET_ID: &str = "bge-small-en-v1.5";

/// The five environment variables that override the STT model paths, all or
/// nothing.
const STT_ENV_VARS: [&str; 5] = [
    "PARAKEET_ENCODER",
    "PARAKEET_DECODER",
    "PARAKEET_JOINER",
    "PARAKEET_TOKENS",
    "PARAKEET_VAD_MODEL",
];

/// The embedding model directory override, read by `BgeConfig::from_env`.
const EMBED_ENV_VAR: &str = "KODABI_EMBED_MODEL_DIR";

/// The file names the STT engine constructor asks for, by manifest set.
const ENCODER_FILE: &str = "encoder.int8.onnx";
const DECODER_FILE: &str = "decoder.int8.onnx";
const JOINER_FILE: &str = "joiner.int8.onnx";
const TOKENS_FILE: &str = "tokens.txt";
const VAD_FILE: &str = "silero_vad.onnx";

/// The five paths `ParakeetConfig` needs.
///
/// Defaults to empty paths, which is deliberately the same thing an unset
/// environment variable used to produce: the engine's own `require_file` check
/// then fails with a `ModelLoad` error naming the file, rather than this module
/// panicking or inventing a path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolvedSttPaths {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: PathBuf,
    pub tokens: PathBuf,
    pub vad_model: PathBuf,
}

/// The Cargo features this binary was built with, as the manifest names them.
///
/// A debug build with no engine feature returns an empty list, which makes every
/// model set unnecessary and the whole app trivially "ready" — correct, because
/// such a build transcribes with `MockEngine` and reads no model at all.
pub(crate) fn enabled_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    if cfg!(feature = "parakeet") {
        features.push("parakeet");
    }
    if cfg!(feature = "embed") {
        features.push("embed");
    }
    features
}

/// Whether a developer has pointed this model set somewhere of their own.
///
/// Passed into the core status and plan functions, which never read the
/// environment themselves.
pub(crate) fn env_overridden(set_id: &str) -> bool {
    match set_id {
        // Silero ships with the STT set and is one of the five overridden paths,
        // so the two stand or fall together.
        PARAKEET_SET_ID | SILERO_SET_ID => stt_env_paths().is_some(),
        EMBED_SET_ID => non_empty(EMBED_ENV_VAR).is_some(),
        _ => false,
    }
}

/// The STT paths, environment first.
pub(crate) fn resolve_stt_paths(app: &AppHandle) -> ResolvedSttPaths {
    if let Some(paths) = stt_env_paths() {
        return paths;
    }
    match sandbox::models_dir(app) {
        Ok(dir) => stt_paths_in(&dir),
        Err(err) => {
            eprintln!("kodabi: {err}");
            ResolvedSttPaths::default()
        }
    }
}

/// The embedding model directory, environment first.
///
/// Returns `None` when neither source resolves, leaving `BgeConfig`'s own empty
/// directory error to explain itself.
#[cfg(feature = "embed")]
pub(crate) fn resolve_embed_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Some(dir) = non_empty(EMBED_ENV_VAR) {
        return Some(PathBuf::from(dir));
    }
    match sandbox::models_dir(app) {
        Ok(dir) => embed_dir_in(&dir),
        Err(err) => {
            eprintln!("kodabi: {err}");
            None
        }
    }
}

/// All five STT overrides, or none of them.
fn stt_env_paths() -> Option<ResolvedSttPaths> {
    let values: Vec<Option<std::ffi::OsString>> =
        STT_ENV_VARS.iter().map(|var| non_empty(var)).collect();
    let set = values.iter().filter(|value| value.is_some()).count();
    if set == 0 {
        return None;
    }
    if set < STT_ENV_VARS.len() {
        // Mixing sources inside one engine is worse than ignoring the override,
        // so say what is missing and fall through to the models directory.
        let missing: Vec<&str> = STT_ENV_VARS
            .iter()
            .zip(&values)
            .filter(|(_, value)| value.is_none())
            .map(|(var, _)| *var)
            .collect();
        eprintln!(
            "kodabi: ignoring a partial model override; these are unset: {}. \
             Set all five or none.",
            missing.join(", ")
        );
        return None;
    }
    let mut paths = values
        .into_iter()
        .map(|value| PathBuf::from(value.expect("all set")));
    Some(ResolvedSttPaths {
        encoder: paths.next().expect("five values"),
        decoder: paths.next().expect("five values"),
        joiner: paths.next().expect("five values"),
        tokens: paths.next().expect("five values"),
        vad_model: paths.next().expect("five values"),
    })
}

/// Derives the STT paths from the manifest, so this module never restates the
/// on-disk layout.
fn stt_paths_in(models_dir: &Path) -> ResolvedSttPaths {
    let Ok(manifest) = kodabi_core::models::manifest::embedded() else {
        return ResolvedSttPaths::default();
    };
    let locate = |set_id: &str, name: &str| -> PathBuf {
        manifest
            .set(set_id)
            .and_then(|set| {
                set.files
                    .iter()
                    .find(|file| file.name == name)
                    .map(|file| file.target_path(models_dir, set))
            })
            .unwrap_or_default()
    };
    ResolvedSttPaths {
        encoder: locate(PARAKEET_SET_ID, ENCODER_FILE),
        decoder: locate(PARAKEET_SET_ID, DECODER_FILE),
        joiner: locate(PARAKEET_SET_ID, JOINER_FILE),
        tokens: locate(PARAKEET_SET_ID, TOKENS_FILE),
        vad_model: locate(SILERO_SET_ID, VAD_FILE),
    }
}

/// The directory holding the embedding model's files, per the manifest.
#[cfg(any(feature = "embed", test))]
fn embed_dir_in(models_dir: &Path) -> Option<PathBuf> {
    let manifest = kodabi_core::models::manifest::embedded().ok()?;
    manifest.set(EMBED_SET_ID).map(|set| set.dir_in(models_dir))
}

/// Reads an environment variable, treating empty as unset — the convention every
/// seam in this app follows.
fn non_empty(key: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(key).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stt_paths_follow_the_manifest_layout() {
        let dir = Path::new(r"C:\app\.models");
        let paths = stt_paths_in(dir);
        let parakeet = dir.join(PARAKEET_SET_ID);
        assert_eq!(paths.encoder, parakeet.join(ENCODER_FILE));
        assert_eq!(paths.decoder, parakeet.join(DECODER_FILE));
        assert_eq!(paths.joiner, parakeet.join(JOINER_FILE));
        assert_eq!(paths.tokens, parakeet.join(TOKENS_FILE));
        // Silero sits at the models-dir root: its manifest set has no subdir.
        assert_eq!(paths.vad_model, dir.join(VAD_FILE));
    }

    #[test]
    fn the_embed_directory_follows_the_manifest_layout() {
        let dir = Path::new(r"C:\app\.models");
        assert_eq!(embed_dir_in(dir), Some(dir.join(EMBED_SET_ID)));
    }

    /// Every file the resolver looks up must exist in the manifest, or the app
    /// would silently resolve to an empty path and fail at engine load.
    #[test]
    fn every_resolved_file_is_named_by_the_manifest() {
        let paths = stt_paths_in(Path::new("base"));
        for path in [
            &paths.encoder,
            &paths.decoder,
            &paths.joiner,
            &paths.tokens,
            &paths.vad_model,
        ] {
            assert!(
                path.starts_with("base"),
                "{path:?} did not resolve from the manifest"
            );
        }
    }

    #[test]
    fn set_ids_match_the_manifest() {
        let manifest = kodabi_core::models::manifest::embedded().expect("parses");
        for id in [PARAKEET_SET_ID, SILERO_SET_ID, EMBED_SET_ID] {
            assert!(manifest.set(id).is_some(), "{id} is not in the manifest");
        }
    }

    #[test]
    fn a_build_without_engine_features_needs_no_model_sets() {
        // `enabled_features` reads this binary's own cfg, so assert the property
        // that matters rather than a fixed list: whatever is compiled in, the
        // manifest must recognise it.
        let manifest = kodabi_core::models::manifest::embedded().expect("parses");
        for feature in enabled_features() {
            assert!(
                manifest
                    .model_sets
                    .iter()
                    .any(|set| set.requires_any.iter().any(|name| name == feature)),
                "no model set declares the compiled feature {feature}"
            );
        }
    }
}
