//! The model manifest: which files a build needs, where they come from, and
//! what they must hash to.
//!
//! The document is checked into the repo and compiled in with `include_str!`
//! rather than fetched or read from disk. Two reasons: the filenames are already
//! load-bearing in code (the engine constructor asks for `encoder.int8.onnx` by
//! name), and a model bump therefore ships with an app update anyway — so a
//! manifest that could be absent, stale, or newer than the code buys a family of
//! failure states for nothing. Bumping models means editing this file and
//! publishing a new `models-v?` release together.
//!
//! Assets live on a GitHub release owned by this repo rather than hotlinked from
//! upstream, so a rename or a retention policy elsewhere cannot break every
//! install. Release assets are a flat namespace, so `asset` (the published name,
//! prefixed by its set) and `name` (the on-disk name the engine expects) differ.

use std::path::{Path, PathBuf};

use super::ModelsError;

/// The schema this build understands. A manifest declaring anything else is
/// refused rather than parsed optimistically.
const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// The compiled-in manifest document.
const EMBEDDED: &str = include_str!("manifest.json");

/// Everything a build might need to download, across all feature combinations.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    /// The release these assets are published under. Informational: it appears
    /// in the NOTICE file and in the upload script's argument checking.
    pub release_tag: String,
    /// Everything before the asset name, with no trailing slash.
    pub base_url: String,
    pub model_sets: Vec<ModelSet>,
}

/// One coherent group of files: a model plus whatever it cannot load without.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModelSet {
    pub id: String,
    /// The Cargo features that make this set necessary. A build compiled with
    /// none of them never downloads it and never reports it missing, which is
    /// what makes a mock (feature-less debug) build trivially ready.
    pub requires_any: Vec<String>,
    /// Subdirectory under the models dir, or empty to sit at its root.
    pub dir: String,
    pub license: License,
    pub files: Vec<ModelFile>,
}

/// Attribution for one set, reproduced into `NOTICE.txt` beside the files.
/// Parakeet is CC-BY-4.0, which obliges us to carry it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct License {
    pub name: String,
    pub notice: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ModelFile {
    /// The on-disk filename, which is what the engine constructor expects.
    pub name: String,
    /// The published release-asset name: flat, and prefixed by its set so two
    /// sets can both contain a `model.onnx`.
    pub asset: String,
    pub size: u64,
    pub sha256: String,
    /// An escape hatch for a file that has to come from somewhere else. Unset
    /// for every file today; honoured ahead of `base_url` when present.
    #[serde(default)]
    pub url: Option<String>,
}

/// The manifest compiled into this binary.
pub fn embedded() -> Result<Manifest, ModelsError> {
    parse(EMBEDDED)
}

/// Parses and validates a manifest document.
pub fn parse(json: &str) -> Result<Manifest, ModelsError> {
    let manifest: Manifest =
        serde_json::from_str(json).map_err(|err| ModelsError::Manifest(err.to_string()))?;
    if manifest.schema_version != SUPPORTED_SCHEMA_VERSION {
        return Err(ModelsError::Manifest(format!(
            "schema version {} is not supported by this build (expected {SUPPORTED_SCHEMA_VERSION})",
            manifest.schema_version
        )));
    }
    if manifest.base_url.ends_with('/') {
        return Err(ModelsError::Manifest(
            "base_url must not end with a slash".to_string(),
        ));
    }
    // Asset names share one flat namespace on the release, so a duplicate would
    // mean two different files silently resolving to the same download.
    let mut assets: Vec<&str> = manifest
        .model_sets
        .iter()
        .flat_map(|set| set.files.iter().map(|file| file.asset.as_str()))
        .collect();
    assets.sort_unstable();
    if let Some(duplicate) = assets.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(ModelsError::Manifest(format!(
            "duplicate asset name {}",
            duplicate[0]
        )));
    }
    Ok(manifest)
}

impl Manifest {
    /// The sets this build actually needs, in manifest order.
    pub fn required_sets(&self, enabled_features: &[&str]) -> Vec<&ModelSet> {
        self.model_sets
            .iter()
            .filter(|set| set.required_for(enabled_features))
            .collect()
    }

    pub fn set(&self, id: &str) -> Option<&ModelSet> {
        self.model_sets.iter().find(|set| set.id == id)
    }
}

impl ModelSet {
    /// Whether a build with these features needs this set.
    pub fn required_for(&self, enabled_features: &[&str]) -> bool {
        self.requires_any
            .iter()
            .any(|required| enabled_features.contains(&required.as_str()))
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// Where this set's files live under the models directory.
    pub fn dir_in(&self, models_dir: &Path) -> PathBuf {
        if self.dir.is_empty() {
            models_dir.to_path_buf()
        } else {
            models_dir.join(&self.dir)
        }
    }

    /// The path shown to a human: `set-dir/file.onnx`, stable across machines,
    /// so progress and error messages never leak a home directory.
    pub fn display_path(&self, file: &ModelFile) -> String {
        if self.dir.is_empty() {
            file.name.clone()
        } else {
            format!("{}/{}", self.dir, file.name)
        }
    }
}

impl ModelFile {
    pub fn url(&self, base_url: &str) -> String {
        match &self.url {
            Some(url) => url.clone(),
            None => format!("{base_url}/{}", self.asset),
        }
    }

    pub fn target_path(&self, models_dir: &Path, set: &ModelSet) -> PathBuf {
        set.dir_in(models_dir).join(&self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_manifest_parses() {
        let manifest = embedded().expect("the shipped manifest must parse");
        assert_eq!(manifest.schema_version, SUPPORTED_SCHEMA_VERSION);
        assert_eq!(manifest.model_sets.len(), 3);
    }

    #[test]
    fn every_embedded_file_declares_a_plausible_hash_and_size() {
        let manifest = embedded().expect("parses");
        for set in &manifest.model_sets {
            assert!(!set.license.notice.is_empty(), "{} has no notice", set.id);
            for file in &set.files {
                assert_eq!(file.sha256.len(), 64, "{}: {}", set.id, file.name);
                assert!(
                    file.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                    "{}: {}",
                    set.id,
                    file.name
                );
                assert!(file.size > 0, "{}: {}", set.id, file.name);
            }
        }
    }

    /// The engine constructor and `BgeConfig` ask for these names literally, so
    /// a rename here is a silent `ModelLoad` failure at first transcription.
    #[test]
    fn the_embedded_manifest_names_the_files_the_engines_expect() {
        let manifest = embedded().expect("parses");
        let names = |id: &str| -> Vec<String> {
            manifest
                .set(id)
                .expect(id)
                .files
                .iter()
                .map(|file| file.name.clone())
                .collect()
        };
        assert_eq!(
            names("parakeet-tdt-0.6b-v2-int8"),
            [
                "encoder.int8.onnx",
                "decoder.int8.onnx",
                "joiner.int8.onnx",
                "tokens.txt"
            ]
        );
        assert_eq!(names("silero-vad"), ["silero_vad.onnx"]);
        assert_eq!(
            names("bge-small-en-v1.5"),
            [
                "model.onnx",
                "tokenizer.json",
                "config.json",
                "special_tokens_map.json",
                "tokenizer_config.json"
            ]
        );
    }

    #[test]
    fn feature_filtering_selects_only_what_a_build_needs() {
        let manifest = embedded().expect("parses");
        let ids = |features: &[&str]| -> Vec<String> {
            manifest
                .required_sets(features)
                .iter()
                .map(|set| set.id.clone())
                .collect()
        };
        assert_eq!(
            ids(&["parakeet"]),
            ["parakeet-tdt-0.6b-v2-int8", "silero-vad"]
        );
        assert_eq!(ids(&["embed"]), ["bge-small-en-v1.5"]);
        assert_eq!(ids(&["parakeet", "embed"]).len(), 3);
        // A mock build compiles no engine feature and therefore needs nothing.
        assert!(ids(&[]).is_empty());
    }

    #[test]
    fn a_wrong_schema_version_is_refused() {
        let err = parse(
            r#"{"schema_version":2,"release_tag":"x","base_url":"https://e","model_sets":[]}"#,
        )
        .expect_err("should refuse");
        assert!(err.to_string().contains("schema version 2"), "{err}");
    }

    #[test]
    fn a_trailing_slash_in_the_base_url_is_refused() {
        let err = parse(
            r#"{"schema_version":1,"release_tag":"x","base_url":"https://e/","model_sets":[]}"#,
        )
        .expect_err("should refuse");
        assert!(err.to_string().contains("slash"), "{err}");
    }

    #[test]
    fn a_duplicate_asset_name_is_refused() {
        let json = r#"{
            "schema_version": 1, "release_tag": "x", "base_url": "https://e",
            "model_sets": [
              {"id":"a","requires_any":["f"],"dir":"a","license":{"name":"MIT","notice":"n"},
               "files":[{"name":"model.onnx","asset":"same.onnx","size":1,"sha256":"ab"}]},
              {"id":"b","requires_any":["f"],"dir":"b","license":{"name":"MIT","notice":"n"},
               "files":[{"name":"model.onnx","asset":"same.onnx","size":1,"sha256":"cd"}]}
            ]}"#;
        let err = parse(json).expect_err("should refuse");
        assert!(err.to_string().contains("duplicate asset"), "{err}");
    }

    #[test]
    fn urls_and_target_paths_follow_the_documented_layout() {
        let manifest = embedded().expect("parses");
        let models_dir = Path::new(r"C:\app\.models");

        let parakeet = manifest.set("parakeet-tdt-0.6b-v2-int8").expect("present");
        let encoder = &parakeet.files[0];
        assert_eq!(
            encoder.url(&manifest.base_url),
            "https://github.com/ShaneHoffman/kodabi/releases/download/models-v1/parakeet-tdt-0.6b-v2-int8.encoder.int8.onnx"
        );
        assert_eq!(
            encoder.target_path(models_dir, parakeet),
            models_dir
                .join("parakeet-tdt-0.6b-v2-int8")
                .join("encoder.int8.onnx")
        );
        assert_eq!(
            parakeet.display_path(encoder),
            "parakeet-tdt-0.6b-v2-int8/encoder.int8.onnx"
        );

        // An empty `dir` puts the file at the models-dir root.
        let silero = manifest.set("silero-vad").expect("present");
        assert_eq!(
            silero.files[0].target_path(models_dir, silero),
            models_dir.join("silero_vad.onnx")
        );
        assert_eq!(silero.display_path(&silero.files[0]), "silero_vad.onnx");
    }

    #[test]
    fn a_per_file_url_override_wins_over_the_base_url() {
        let json = r#"{
            "schema_version": 1, "release_tag": "x", "base_url": "https://e",
            "model_sets": [{"id":"a","requires_any":["f"],"dir":"","license":{"name":"MIT","notice":"n"},
              "files":[{"name":"m.onnx","asset":"m.onnx","size":1,"sha256":"ab","url":"https://elsewhere/m.onnx"}]}]}"#;
        let manifest = parse(json).expect("parses");
        assert_eq!(
            manifest.model_sets[0].files[0].url(&manifest.base_url),
            "https://elsewhere/m.onnx"
        );
    }
}
