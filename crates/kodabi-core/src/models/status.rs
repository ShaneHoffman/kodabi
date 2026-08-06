//! Is the app provisioned? Answered by name and byte length only.
//!
//! This runs whenever a view asks, so it must stay cheap: hashing 631 MB to
//! answer "can I transcribe?" would cost seconds on every launch. The trade is
//! explicit — a file that was corrupted in place while keeping its exact length
//! passes here and fails loudly later at engine load, where re-downloading is
//! the remedy. SHA-256 is checked once, at the moment a download finalizes,
//! which is the only point where a wrong file can actually be introduced.

use std::path::Path;

use super::manifest::{Manifest, ModelSet};

/// The state of one model set on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SetState {
    /// Every file is present at its declared size.
    Ready,
    /// A developer pointed the app at their own copy through the environment,
    /// so this set is not ours to manage. Never downloaded, never reported
    /// missing.
    EnvOverridden,
    /// Nothing of this set is on disk.
    Missing,
    /// Some files, or some bytes of a resumable file, are present.
    Partial,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetStatus {
    pub id: String,
    pub state: SetState,
    pub bytes_total: u64,
    pub bytes_present: u64,
    /// The licence name, so the UI can attribute without parsing the notice.
    pub license: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStatus {
    /// Whether every set this build needs is usable. True for a build that
    /// compiled no engine feature, which needs no models at all.
    pub ready: bool,
    /// Bytes still to fetch across every set that is not ready. Excludes
    /// environment-overridden sets: the app will not download those.
    pub bytes_required: u64,
    /// Of `bytes_required`, how many are already on disk (finished files plus
    /// the length of any resumable partial), so a resumed download can report
    /// honest totals rather than restarting its arithmetic.
    pub bytes_present: u64,
    pub sets: Vec<SetStatus>,
}

/// Inspects the models directory.
///
/// `env_overridden` is supplied by the shell because this module does not read
/// the environment; it answers "has a developer pointed this set elsewhere?".
pub fn status(
    manifest: &Manifest,
    models_dir: &Path,
    enabled_features: &[&str],
    env_overridden: &dyn Fn(&str) -> bool,
) -> ModelsStatus {
    let mut sets = Vec::new();
    let mut bytes_required = 0;
    let mut bytes_present = 0;
    let mut ready = true;

    for set in manifest.required_sets(enabled_features) {
        let total = set.total_size();
        if env_overridden(&set.id) {
            sets.push(SetStatus {
                id: set.id.clone(),
                state: SetState::EnvOverridden,
                bytes_total: total,
                bytes_present: total,
                license: set.license.name.clone(),
            });
            continue;
        }

        let present = present_bytes(set, models_dir);
        let state = if present.complete_files == set.files.len() {
            SetState::Ready
        } else if present.bytes == 0 {
            SetState::Missing
        } else {
            SetState::Partial
        };
        if state != SetState::Ready {
            ready = false;
            bytes_required += total;
            bytes_present += present.bytes;
        }
        sets.push(SetStatus {
            id: set.id.clone(),
            state,
            bytes_total: total,
            bytes_present: present.bytes,
            license: set.license.name.clone(),
        });
    }

    ModelsStatus {
        ready,
        bytes_required,
        bytes_present,
        sets,
    }
}

struct Present {
    bytes: u64,
    complete_files: usize,
}

/// Counts what is on disk for one set: finished files at their exact declared
/// size, plus the length of any `.part` a previous run left behind.
fn present_bytes(set: &ModelSet, models_dir: &Path) -> Present {
    let mut bytes = 0;
    let mut complete_files = 0;
    for file in &set.files {
        let target = file.target_path(models_dir, set);
        if file_len(&target) == Some(file.size) {
            bytes += file.size;
            complete_files += 1;
            continue;
        }
        // A partial download counts toward progress but never toward readiness.
        if let Some(partial) = file_len(&super::download::part_path(&target)) {
            bytes += partial.min(file.size);
        }
    }
    Present {
        bytes,
        complete_files,
    }
}

fn file_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::manifest;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    fn never_overridden(_: &str) -> bool {
        false
    }

    /// Writes a file of exactly `len` bytes at `path`.
    fn write_sized(path: &PathBuf, len: u64) {
        fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
        let mut file = fs::File::create(path).expect("create");
        file.write_all(&vec![0u8; len as usize]).expect("write");
    }

    /// A two-set manifest with small files, so tests can materialise them.
    fn test_manifest() -> Manifest {
        manifest::parse(
            r#"{
              "schema_version": 1, "release_tag": "t", "base_url": "https://e",
              "model_sets": [
                {"id":"stt","requires_any":["parakeet"],"dir":"stt",
                 "license":{"name":"CC-BY-4.0","notice":"n"},
                 "files":[{"name":"a.onnx","asset":"stt.a.onnx","size":10,"sha256":"aa"},
                          {"name":"b.txt","asset":"stt.b.txt","size":4,"sha256":"bb"}]},
                {"id":"emb","requires_any":["embed"],"dir":"",
                 "license":{"name":"MIT","notice":"n"},
                 "files":[{"name":"e.onnx","asset":"emb.e.onnx","size":6,"sha256":"cc"}]}
              ]}"#,
        )
        .expect("parses")
    }

    #[test]
    fn a_build_with_no_engine_features_needs_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = status(&test_manifest(), dir.path(), &[], &never_overridden);
        assert!(status.ready);
        assert_eq!(status.bytes_required, 0);
        assert!(status.sets.is_empty());
    }

    #[test]
    fn an_empty_directory_reports_every_required_set_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = status(
            &test_manifest(),
            dir.path(),
            &["parakeet", "embed"],
            &never_overridden,
        );
        assert!(!status.ready);
        assert_eq!(status.sets.len(), 2);
        assert!(status.sets.iter().all(|set| set.state == SetState::Missing));
        assert_eq!(status.bytes_required, 20);
        assert_eq!(status.bytes_present, 0);
    }

    #[test]
    fn every_file_at_its_declared_size_is_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_sized(&dir.path().join("stt").join("a.onnx"), 10);
        write_sized(&dir.path().join("stt").join("b.txt"), 4);

        let status = status(
            &test_manifest(),
            dir.path(),
            &["parakeet"],
            &never_overridden,
        );
        assert!(status.ready);
        assert_eq!(status.sets[0].state, SetState::Ready);
        assert_eq!(status.bytes_required, 0);
    }

    #[test]
    fn a_file_of_the_wrong_size_does_not_count_as_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_sized(&dir.path().join("stt").join("a.onnx"), 9);
        write_sized(&dir.path().join("stt").join("b.txt"), 4);

        let status = status(
            &test_manifest(),
            dir.path(),
            &["parakeet"],
            &never_overridden,
        );
        assert!(!status.ready);
        assert_eq!(status.sets[0].state, SetState::Partial);
        // Only the correctly sized sibling counted.
        assert_eq!(status.sets[0].bytes_present, 4);
    }

    #[test]
    fn a_resumable_partial_counts_toward_progress_but_not_readiness() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_sized(&dir.path().join("stt").join("a.onnx.part"), 6);

        let status = status(
            &test_manifest(),
            dir.path(),
            &["parakeet"],
            &never_overridden,
        );
        assert_eq!(status.sets[0].state, SetState::Partial);
        assert_eq!(status.sets[0].bytes_present, 6);
        assert_eq!(status.bytes_present, 6);
        assert_eq!(status.bytes_required, 14);
        assert!(!status.ready);
    }

    #[test]
    fn an_overlong_partial_cannot_inflate_progress_past_the_declared_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_sized(&dir.path().join("stt").join("a.onnx.part"), 999);

        let status = status(
            &test_manifest(),
            dir.path(),
            &["parakeet"],
            &never_overridden,
        );
        assert_eq!(status.sets[0].bytes_present, 10);
    }

    #[test]
    fn an_environment_overridden_set_is_ready_and_never_downloaded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = status(
            &test_manifest(),
            dir.path(),
            &["parakeet", "embed"],
            &|id| id == "stt",
        );
        assert!(!status.ready, "emb is still missing");
        assert_eq!(status.sets[0].state, SetState::EnvOverridden);
        assert_eq!(status.sets[1].state, SetState::Missing);
        // The overridden set contributes nothing to what we would fetch.
        assert_eq!(status.bytes_required, 6);
    }

    #[test]
    fn overriding_every_required_set_reports_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = status(&test_manifest(), dir.path(), &["parakeet"], &|_| true);
        assert!(status.ready);
        assert_eq!(status.bytes_required, 0);
    }

    #[test]
    fn a_directory_where_a_file_should_be_is_not_mistaken_for_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("stt").join("a.onnx")).expect("mkdir");
        let status = status(
            &test_manifest(),
            dir.path(),
            &["parakeet"],
            &never_overridden,
        );
        assert_eq!(status.sets[0].state, SetState::Missing);
    }
}
