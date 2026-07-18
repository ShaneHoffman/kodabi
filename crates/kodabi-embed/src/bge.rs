//! bge-small-en-v1.5 embedding via `fastembed`.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use fastembed::{
    InitOptionsUserDefined, Pooling, QuantizationMode, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use kodabi_core::embed::{l2_normalize, EmbedError, Embedder};
use kodabi_core::index::EMBEDDING_DIM;

/// bge-small-en-v1.5's output dimensionality.
pub const BGE_DIM: usize = 384;

// The index schema and this model must agree on the vector width. A mismatch is
// a `notes_vec` corruption waiting to happen, so catch it at compile time.
const _: () = assert!(BGE_DIM == EMBEDDING_DIM);

/// bge v1.5's asymmetric query instruction. Passages are embedded bare; queries
/// carry this prefix (see the [`Embedder`] contract). Callers never apply it.
const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

/// The model's token window. Longer inputs are truncated by the tokenizer;
/// `kodabi_core::embed::chunk_body` keeps chunks well under this.
const MODEL_MAX_LENGTH: usize = 512;

/// Default intra-op thread count — deliberately low to stay within the Phase 1
/// resource budget (docs/RESOURCE_BUDGET.md); override via `KODABI_EMBED_THREADS`.
const DEFAULT_THREADS: usize = 1;

/// Upper bound on the thread override, so a stray large value can't monopolize
/// the machine mid-capture.
const MAX_THREADS: usize = 8;

/// The bge model's file names, expected under [`BgeConfig::model_dir`].
const ONNX_FILE: &str = "model.onnx";
const TOKENIZER_FILE: &str = "tokenizer.json";
const CONFIG_FILE: &str = "config.json";
const SPECIAL_TOKENS_MAP_FILE: &str = "special_tokens_map.json";
const TOKENIZER_CONFIG_FILE: &str = "tokenizer_config.json";

/// Where the model lives and how many threads it may use.
#[derive(Debug, Clone)]
pub struct BgeConfig {
    /// Directory holding `model.onnx` and the four tokenizer files.
    pub model_dir: PathBuf,
    /// ONNX Runtime intra-op threads.
    pub intra_threads: usize,
}

impl BgeConfig {
    /// Reads configuration from the environment: `KODABI_EMBED_MODEL_DIR` (the
    /// model directory) and `KODABI_EMBED_THREADS` (thread count, clamped to
    /// `1..=8`, defaulting to `1` when unset or unparseable). Mirrors the
    /// env-based model-path convention `kodabi-transcribe` uses.
    pub fn from_env() -> Self {
        let model_dir = std::env::var_os("KODABI_EMBED_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_default();
        let intra_threads = std::env::var("KODABI_EMBED_THREADS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(clamp_threads)
            .unwrap_or(DEFAULT_THREADS);
        Self {
            model_dir,
            intra_threads,
        }
    }
}

fn clamp_threads(n: usize) -> usize {
    n.clamp(1, MAX_THREADS)
}

/// A bge-small embedder.
///
/// The ONNX session is loaded lazily on the first embed and kept resident
/// afterward (bge-small is ~150 MB — small enough to hold, unlike the STT
/// engines). A [`Mutex`] serializes inference so only one embed runs at a time,
/// honoring the "one heavyweight model resident at a time" convention, and lets
/// the `&self` [`Embedder`] methods drive `fastembed`'s `&mut self` API. A load
/// failure is cached in the [`OnceLock`] and re-reported on every call rather
/// than retried.
pub struct BgeEmbedder {
    config: BgeConfig,
    model: OnceLock<Result<Mutex<TextEmbedding>, EmbedError>>,
}

impl BgeEmbedder {
    /// Creates an embedder that will load the model on first use.
    pub fn new(config: BgeConfig) -> Self {
        Self {
            config,
            model: OnceLock::new(),
        }
    }

    /// The resident session, loading it on first call. A cached load error is
    /// cloned and returned.
    fn model(&self) -> Result<&Mutex<TextEmbedding>, EmbedError> {
        self.model
            .get_or_init(|| load_model(&self.config))
            .as_ref()
            .map_err(Clone::clone)
    }

    /// Embeds already-prepared inputs (prefixing/normalization handled by the
    /// caller), returning one L2-normalized `BGE_DIM` vector each.
    fn embed_inputs(&self, inputs: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedError> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let mutex = self.model()?;
        // A poisoned lock still holds a valid model — a prior embed panicking
        // (or being cancelled) doesn't corrupt the ONNX session — so recover it
        // rather than propagate the poison.
        let mut model = mutex.lock().unwrap_or_else(|poison| poison.into_inner());
        let mut vectors = model
            .embed(inputs, None)
            .map_err(|err| EmbedError::Backend(format!("bge inference failed: {err}")))?;
        for vector in &mut vectors {
            validate_and_normalize(vector)?;
        }
        Ok(vectors)
    }
}

impl Embedder for BgeEmbedder {
    fn dim(&self) -> usize {
        BGE_DIM
    }

    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        self.embed_inputs(texts.to_vec())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        let prefixed = format!("{BGE_QUERY_PREFIX}{text}");
        self.embed_inputs(vec![prefixed])?
            .pop()
            .ok_or_else(|| EmbedError::Backend("bge returned no query vector".to_string()))
    }
}

/// Loads the model from `config.model_dir`, reading every file into memory (the
/// `fastembed` user-defined API takes bytes, not paths). Any missing file
/// yields a clear [`EmbedError::Backend`] naming it — no network is touched.
fn load_model(config: &BgeConfig) -> Result<Mutex<TextEmbedding>, EmbedError> {
    if config.model_dir.as_os_str().is_empty() {
        return Err(EmbedError::Backend(
            "no embedding model directory configured (set KODABI_EMBED_MODEL_DIR)".to_string(),
        ));
    }

    let dir = &config.model_dir;
    let onnx_file = read_required(dir, ONNX_FILE)?;
    let tokenizer_files = TokenizerFiles {
        tokenizer_file: read_required(dir, TOKENIZER_FILE)?,
        config_file: read_required(dir, CONFIG_FILE)?,
        special_tokens_map_file: read_required(dir, SPECIAL_TOKENS_MAP_FILE)?,
        tokenizer_config_file: read_required(dir, TOKENIZER_CONFIG_FILE)?,
    };

    let model = UserDefinedEmbeddingModel::new(onnx_file, tokenizer_files)
        .with_pooling(Pooling::Cls)
        .with_quantization(QuantizationMode::None);
    let options = InitOptionsUserDefined::new()
        .with_max_length(MODEL_MAX_LENGTH)
        .with_intra_threads(config.intra_threads);

    let embedding = TextEmbedding::try_new_from_user_defined(model, options)
        .map_err(|err| EmbedError::Backend(format!("failed to initialize bge-small: {err}")))?;
    Ok(Mutex::new(embedding))
}

fn read_required(dir: &Path, file: &str) -> Result<Vec<u8>, EmbedError> {
    let path = dir.join(file);
    std::fs::read(&path).map_err(|err| {
        EmbedError::Backend(format!(
            "cannot read embedding model file {}: {err}",
            path.display()
        ))
    })
}

/// Enforces the `BGE_DIM` contract and normalizes to unit length. `fastembed`'s
/// normalization of user-defined models is version-dependent, so normalizing
/// here makes stored vectors unit-length regardless (L2 distance ≡ cosine).
fn validate_and_normalize(vector: &mut [f32]) -> Result<(), EmbedError> {
    if vector.len() != BGE_DIM {
        return Err(EmbedError::Backend(format!(
            "bge returned a {}-dim vector, expected {BGE_DIM}",
            vector.len()
        )));
    }
    l2_normalize(vector);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_threads_holds_the_1_to_8_range() {
        assert_eq!(clamp_threads(0), 1);
        assert_eq!(clamp_threads(1), 1);
        assert_eq!(clamp_threads(4), 4);
        assert_eq!(clamp_threads(99), MAX_THREADS);
    }

    #[test]
    fn an_unconfigured_model_dir_fails_cleanly_without_panicking() {
        let embedder = BgeEmbedder::new(BgeConfig {
            model_dir: PathBuf::new(),
            intra_threads: 1,
        });
        let err = embedder.embed_query("anything").unwrap_err();
        let EmbedError::Backend(message) = err;
        assert!(
            message.contains("KODABI_EMBED_MODEL_DIR"),
            "error should name the env var, got: {message}"
        );
    }

    #[test]
    fn a_missing_model_file_names_the_path() {
        let dir = std::env::temp_dir().join("kodabi-embed-no-such-model-dir");
        let embedder = BgeEmbedder::new(BgeConfig {
            model_dir: dir,
            intra_threads: 1,
        });
        let err = embedder.embed_query("anything").unwrap_err();
        let EmbedError::Backend(message) = err;
        assert!(
            message.contains(ONNX_FILE),
            "error should name the missing file, got: {message}"
        );
    }
}
