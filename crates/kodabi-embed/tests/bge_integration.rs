//! End-to-end checks against the real bge-small model.
//!
//! These are `#[ignore]`d and gated on `KODABI_EMBED_MODEL_DIR` — they need the
//! model files on disk, so they never run in the default `cargo test` sweep or
//! in CI. Run them explicitly:
//!
//! ```powershell
//! $env:KODABI_EMBED_MODEL_DIR = "C:\models\bge-small-en-v1.5"
//! cargo test -p kodabi-embed --features bge -- --ignored --nocapture
//! ```
#![cfg(feature = "bge")]

use kodabi_core::embed::{index_note, Embedder};
use kodabi_core::index::{IndexedNote, NoteIndex, NoteType};
use kodabi_embed::{BgeConfig, BgeEmbedder, BGE_DIM};

/// Builds an embedder from the env-configured model dir, or `None` to skip.
fn embedder() -> Option<BgeEmbedder> {
    let dir = std::env::var_os("KODABI_EMBED_MODEL_DIR")?;
    if dir.is_empty() {
        return None;
    }
    Some(BgeEmbedder::new(BgeConfig {
        model_dir: dir.into(),
        intra_threads: 1,
    }))
}

fn indexed(id: &str, title: &str, body: &str) -> IndexedNote {
    IndexedNote {
        id: id.to_string(),
        path: format!("Acme/{id}.md"),
        title: title.to_string(),
        note_type: NoteType::Note,
        project: Some("Acme".to_string()),
        date: "2026-07-11".to_string(),
        tags: vec![],
        source: "manual".to_string(),
        confidence: None,
        body: body.to_string(),
        meeting: None,
    }
}

#[test]
#[ignore = "requires KODABI_EMBED_MODEL_DIR pointing at a local bge-small-en-v1.5 model"]
fn embeddings_are_384_dim_unit_length_and_deterministic() {
    let Some(embedder) = embedder() else {
        eprintln!("skipping: KODABI_EMBED_MODEL_DIR unset");
        return;
    };

    let first = embedder
        .embed_passages(&["a sentence about budgets".to_string()])
        .unwrap();
    assert_eq!(first[0].len(), BGE_DIM);

    let norm = first[0].iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-4,
        "expected unit length, got {norm}"
    );

    // Same text, same machine, fixed threads → bitwise-identical vectors.
    let second = embedder
        .embed_passages(&["a sentence about budgets".to_string()])
        .unwrap();
    assert_eq!(first, second, "embedding must be deterministic");
}

#[test]
#[ignore = "requires KODABI_EMBED_MODEL_DIR pointing at a local bge-small-en-v1.5 model"]
fn semantically_similar_notes_rank_nearer_than_an_unrelated_one() {
    // The task's "done when": writing notes produces stored vectors, and
    // semantically similar notes rank near each other in a vector query.
    let Some(embedder) = embedder() else {
        eprintln!("skipping: KODABI_EMBED_MODEL_DIR unset");
        return;
    };

    let mut index = NoteIndex::open_in_memory().unwrap();
    for note in [
        indexed(
            "n_budget1",
            "Quarterly budget review",
            "Finance planning for Q3 spend and cost controls across the teams.",
        ),
        indexed(
            "n_budget2",
            "Next quarter budget allocations",
            "We discussed how to allocate the budget for the upcoming quarter.",
        ),
        indexed(
            "n_bread",
            "Sourdough starter",
            "Feeding schedule for the sourdough starter: discard half, add flour and water.",
        ),
    ] {
        index_note(&mut index, &note, Some(&embedder)).unwrap();
    }

    // Every note produced at least one stored vector.
    assert!(index.note_has_chunks("n_budget1").unwrap());
    assert!(index.note_has_chunks("n_budget2").unwrap());
    assert!(index.note_has_chunks("n_bread").unwrap());

    let query = embedder
        .embed_query("planning the budget for next quarter")
        .unwrap();
    let hits = index.nearest_chunks(&query, 3).unwrap();

    let distance = |id: &str| {
        hits.iter()
            .find(|hit| hit.note_id == id)
            .unwrap_or_else(|| panic!("{id} missing from KNN results: {hits:?}"))
            .distance
    };
    let bread = distance("n_bread");
    let budget1 = distance("n_budget1");
    let budget2 = distance("n_budget2");

    assert!(
        budget1 < bread && budget2 < bread,
        "budget notes should rank nearer than the unrelated bread note \
         (budget1={budget1}, budget2={budget2}, bread={bread})"
    );
}
