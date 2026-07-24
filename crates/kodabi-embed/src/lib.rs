//! Kodabi's local embedding backend — the heavy, feature-gated half of the
//! embedding pipeline whose pure half lives in `kodabi_core::embed`.
//!
//! Behind the `bge` feature this crate implements [`kodabi_core::embed::Embedder`]
//! with **bge-small-en-v1.5**, a 384-dimensional CLS-pooled sentence encoder,
//! run on CPU through [`fastembed`] (ONNX Runtime). The model is loaded from
//! local files and never fetched over the network at runtime — data custody is
//! a core promise (FOUNDING_DOC §2).
//!
//! # Model files
//!
//! Point `KODABI_EMBED_MODEL_DIR` at a directory holding the ONNX export of
//! bge-small-en-v1.5 — `model.onnx` plus the four tokenizer files
//! (`tokenizer.json`, `config.json`, `special_tokens_map.json`,
//! `tokenizer_config.json`), as published at e.g. Hugging Face
//! `Xenova/bge-small-en-v1.5`. Download-on-first-run and settings-driven paths
//! are a later ticket; today the directory is provided out of band (the
//! benchmark models live in a local models directory).
//!
//! Without the `bge` feature the crate is an empty shell, so the default
//! workspace build pulls in no ONNX toolchain.

#[cfg(feature = "bge")]
mod bge;

#[cfg(feature = "bge")]
pub use bge::{BgeConfig, BgeEmbedder, BGE_DIM};

#[cfg(test)]
mod tests {
    /// The crate compiles and links in the default (no-`bge`) build, giving
    /// `cargo test --workspace` something to run without the ONNX toolchain.
    #[test]
    fn default_build_links() {
        // Nothing to assert — reaching here proves the crate built.
    }
}
