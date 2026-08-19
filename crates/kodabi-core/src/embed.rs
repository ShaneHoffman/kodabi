//! The local embedding pipeline: chunk a note's body, embed each chunk, and
//! store the vectors in the index.
//!
//! Everything here is pure and UI-agnostic. The heavy model lives behind the
//! [`Embedder`] trait (its real implementation is the feature-gated
//! `kodabi-embed` crate); this module owns the deterministic parts — how a body
//! is split into chunks, what text is fed to the model, and how the resulting
//! vectors flow into [`crate::index::NoteIndex`]. A [`FakeEmbedder`] gives the
//! rest of the workspace a dependency-free stand-in for tests and default
//! builds.
//!
//! The model is **bge-small-en-v1.5** (see [`crate::index::EMBEDDING_DIM`]):
//! passages are embedded bare, queries carry an instruction prefix, and every
//! vector is L2-normalized. That passage/query asymmetry is the [`Embedder`]
//! implementation's responsibility — callers never pre-apply a prefix.

use crate::index::{EmbeddedChunk, IndexedNote, NoteIndex, EMBEDDING_DIM};

/// Maximum characters per chunk.
///
/// bge-small truncates input at 512 tokens; at the rough ~4-chars-per-token
/// heuristic that is ~2 000 characters, and this budget of ~400 tokens leaves
/// generous headroom for the title prepended to every chunk (see
/// [`passage_input`]) plus the tokenizer's special tokens. The model truncates
/// anything longer regardless, so overshooting only costs a little recall, not
/// correctness.
pub const MAX_CHUNK_CHARS: usize = 1_600;

/// Splits a note body into chunks for embedding.
///
/// The body is expected frontmatter-stripped (as [`IndexedNote::body`] is).
/// Splitting is paragraph-aware and deterministic: blank lines delimit
/// paragraphs, consecutive paragraphs are greedily packed into a chunk while
/// they fit [`MAX_CHUNK_CHARS`], and a single paragraph larger than the budget
/// is hard-split at whitespace. Chunks are returned in document order; a body
/// that is empty or all whitespace yields no chunks (a title-only note stays
/// findable through full-text search).
pub fn chunk_body(body: &str) -> Vec<String> {
    // Bodies are authored and written on Windows, so normalize line endings
    // before splitting — otherwise a `\r\n` body would chunk differently from
    // the same text with `\n`.
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");

    let paragraphs = normalized
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty());

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();

    for paragraph in paragraphs {
        if char_count(paragraph) > MAX_CHUNK_CHARS {
            // An oversized paragraph can't be packed with neighbours; flush the
            // buffer and emit the paragraph as its own hard-split run.
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            chunks.extend(hard_split(paragraph, MAX_CHUNK_CHARS));
            continue;
        }

        if current.is_empty() {
            current.push_str(paragraph);
        } else if char_count(&current) + 2 + char_count(paragraph) <= MAX_CHUNK_CHARS {
            current.push_str("\n\n");
            current.push_str(paragraph);
        } else {
            chunks.push(std::mem::take(&mut current));
            current.push_str(paragraph);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// The text actually embedded for a chunk: the note title prepended to the
/// chunk body (or the bare chunk when there is no title). Prepending the title
/// gives every chunk note-level context and is why an upsert that only changes
/// the title still invalidates the vectors. The bare chunk — not this — is what
/// is stored in `note_chunks`.
pub fn passage_input(title: &str, chunk: &str) -> String {
    if title.is_empty() {
        chunk.to_string()
    } else {
        format!("{title}\n\n{chunk}")
    }
}

/// Scales `v` to unit L2 length in place. A zero vector is left unchanged (its
/// length is undefined); embedders never produce one from real text.
pub fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Hard-splits a paragraph longer than `budget` chars into pieces each within
/// budget, breaking at the last whitespace before the limit (falling back to a
/// char boundary at the limit when a piece has no interior whitespace). Walks
/// `char_indices` so a split never lands mid-codepoint.
///
/// Crate-visible because [`crate::distill`] needs the same budget-bounded split
/// for an over-long utterance; keeping one implementation keeps the two from
/// drifting.
pub(crate) fn hard_split(paragraph: &str, budget: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = paragraph.trim();

    while char_count(rest) > budget {
        let mut boundary_at_budget = rest.len();
        let mut last_ws: Option<(usize, usize)> = None;
        for (count, (byte_idx, ch)) in rest.char_indices().enumerate() {
            if count == budget {
                boundary_at_budget = byte_idx;
                break;
            }
            if ch.is_whitespace() {
                last_ws = Some((byte_idx, ch.len_utf8()));
            }
        }

        // Prefer the last whitespace inside the window (dropping that
        // whitespace char); with none, cut exactly at the budget boundary.
        let (cut, resume) = match last_ws {
            Some((ws_idx, ws_len)) => (ws_idx, ws_idx + ws_len),
            None => (boundary_at_budget, boundary_at_budget),
        };

        let piece = rest[..cut].trim();
        if !piece.is_empty() {
            pieces.push(piece.to_string());
        }
        rest = rest[resume..].trim_start();
    }

    let tail = rest.trim();
    if !tail.is_empty() {
        pieces.push(tail.to_string());
    }
    pieces
}

/// An error from the embedding backend. `Clone` so an implementation that
/// lazily loads its model can cache and re-report a load failure.
#[derive(Debug, Clone, thiserror::Error)]
pub enum EmbedError {
    /// The backend (model load or inference) failed; the string describes it.
    #[error("embedding backend error: {0}")]
    Backend(String),
}

/// Turns text into vectors for storage and search.
///
/// Contract: implementations own the model's passage/query asymmetry.
/// [`embed_passages`](Embedder::embed_passages) takes bare passage text;
/// [`embed_query`](Embedder::embed_query) applies the model's query instruction
/// prefix internally. Every returned vector is L2-normalized and exactly
/// [`dim`](Embedder::dim) long. Callers never pre-apply a prefix or normalize.
pub trait Embedder: Send + Sync {
    /// The dimensionality of every vector this embedder returns.
    fn dim(&self) -> usize;

    /// Embeds passage texts, one vector per input, in order.
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;

    /// Embeds a single search query.
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError>;
}

/// A deterministic, dependency-free [`Embedder`] for tests and default builds.
///
/// It hashes the input text (FNV-1a) to seed a small PRNG and fills an
/// [`EMBEDDING_DIM`]-length unit vector — no model, no I/O. Same text always
/// yields the same vector, and [`embed_query`](Embedder::embed_query) matches
/// [`embed_passages`](Embedder::embed_passages) for the same text (it applies
/// no prefix), so a note's own text is its own nearest neighbour. The vectors
/// carry no semantic meaning; this exists to exercise the plumbing, not to
/// retrieve well.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeEmbedder;

impl FakeEmbedder {
    fn embed_one(&self, text: &str) -> Vec<f32> {
        // FNV-1a over the bytes, then xorshift64 to expand into components.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut state = hash | 1; // keep the xorshift state nonzero

        let mut v = Vec::with_capacity(EMBEDDING_DIM);
        for _ in 0..EMBEDDING_DIM {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Top 53 bits → a double in [0, 1), then map to [-1, 1).
            let unit = (state >> 11) as f64 / (1u64 << 53) as f64;
            v.push((unit * 2.0 - 1.0) as f32);
        }
        l2_normalize(&mut v);
        v
    }
}

impl Embedder for FakeEmbedder {
    fn dim(&self) -> usize {
        EMBEDDING_DIM
    }

    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Ok(texts.iter().map(|t| self.embed_one(t)).collect())
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
        Ok(self.embed_one(text))
    }
}

/// An error from [`index_note`]: either the index write or the embedder failed.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error(transparent)]
    Index(#[from] crate::index::IndexError),
    #[error(transparent)]
    Embed(#[from] EmbedError),
}

/// The chunks of a note still needing embedding, as produced by
/// [`upsert_and_plan`] and [`plan_from_content`].
///
/// `inputs` are the title-prefixed texts fed to the embedder (see
/// [`passage_input`]); `chunks` are the bare chunk bodies stored beside each
/// vector so a search hit's snippet is the note's own words. The two are
/// parallel and equal-length — `inputs[i]` embeds the vector stored next to
/// `chunks[i]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingEmbeddings {
    /// Title-prefixed embedder inputs, one per chunk.
    pub inputs: Vec<String>,
    /// Bare chunk bodies, one per input, stored alongside the vectors.
    pub chunks: Vec<String>,
}

/// Chunks a note's `body` and builds the embedder inputs for it, or `None` when
/// the body is empty or all whitespace (a title-only note stays findable through
/// full-text search). Pure — no index access — so the app can call it off the
/// index lock and only re-acquire the lock to store the finished vectors.
pub fn plan_from_content(title: &str, body: &str) -> Option<PendingEmbeddings> {
    let chunks = chunk_body(body);
    if chunks.is_empty() {
        return None;
    }
    let inputs = chunks
        .iter()
        .map(|chunk| passage_input(title, chunk))
        .collect();
    Some(PendingEmbeddings { inputs, chunks })
}

/// Upserts `note`, then reports what still needs embedding — `None` when the
/// note's vectors are already current (a pure move kept its chunk rows) or its
/// body is empty.
///
/// This is the lock-holding half of the pipeline: it touches the index but does
/// no embedding, so a caller that serializes index access can run the slow
/// [`Embedder`] call *without* the lock and hand the result to
/// [`store_embeddings`]. It leans on
/// [`upsert_note`](crate::index::NoteIndex::upsert_note)'s invalidation — an
/// upsert that changes the title or body drops the note's chunk rows, a pure
/// move keeps them — so surviving chunk rows mean the vectors are still current.
pub fn upsert_and_plan(
    index: &mut NoteIndex,
    note: &IndexedNote,
) -> Result<Option<PendingEmbeddings>, PipelineError> {
    index.upsert_note(note)?;
    // Surviving chunk rows mean the content is unchanged and the vectors are
    // current — nothing to embed.
    if index.note_has_chunks(&note.id)? {
        return Ok(None);
    }
    Ok(plan_from_content(&note.title, &note.body))
}

/// Stores freshly computed `embeddings` for `note_id`'s `chunks`, replacing any
/// existing chunk rows. The two slices are parallel (see [`PendingEmbeddings`]);
/// a length mismatch means the embedder broke its one-vector-per-input contract
/// and is rejected rather than silently truncated.
pub fn store_embeddings(
    index: &mut NoteIndex,
    note_id: &str,
    chunks: &[String],
    embeddings: Vec<Vec<f32>>,
) -> Result<(), PipelineError> {
    if chunks.len() != embeddings.len() {
        return Err(EmbedError::Backend(format!(
            "embedder returned {} vectors for {} chunks",
            embeddings.len(),
            chunks.len()
        ))
        .into());
    }
    // Store the bare chunk text (not the title-prefixed embedder input) beside
    // each vector, so a search hit's snippet is the note's own words.
    let embedded: Vec<EmbeddedChunk> = chunks
        .iter()
        .cloned()
        .zip(embeddings)
        .map(|(text, embedding)| EmbeddedChunk { text, embedding })
        .collect();
    index.set_note_chunks(note_id, &embedded)?;
    Ok(())
}

/// Indexes a note and (if an embedder is supplied) keeps its embeddings current,
/// in one call against an exclusively-held index.
///
/// This is the single entry point for callers that own the index outright (its
/// tests, and any batch re-index). The app's write path instead composes
/// [`upsert_and_plan`] → embed → [`store_embeddings`] so it can drop the index
/// lock across the slow embed. With no embedder the note is still upserted
/// (full-text search works); only the vectors are skipped. Deterministic for a
/// given (content, model).
pub fn index_note(
    index: &mut NoteIndex,
    note: &IndexedNote,
    embedder: Option<&dyn Embedder>,
) -> Result<(), PipelineError> {
    let Some(embedder) = embedder else {
        index.upsert_note(note)?;
        return Ok(());
    };

    let Some(pending) = upsert_and_plan(index, note)? else {
        return Ok(());
    };
    let embeddings = embedder.embed_passages(&pending.inputs)?;
    store_embeddings(index, &note.id, &pending.chunks, embeddings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::NoteType;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- chunker ------------------------------------------------------------

    #[test]
    fn empty_or_whitespace_body_yields_no_chunks() {
        assert!(chunk_body("").is_empty());
        assert!(chunk_body("   \n\n  \t \n").is_empty());
    }

    #[test]
    fn a_short_body_is_a_single_chunk() {
        assert_eq!(chunk_body("just one paragraph"), vec!["just one paragraph"]);
    }

    #[test]
    fn small_paragraphs_pack_into_one_chunk_joined_by_blank_lines() {
        let body = "first para\n\nsecond para\n\nthird para";
        assert_eq!(
            chunk_body(body),
            vec!["first para\n\nsecond para\n\nthird para"]
        );
    }

    #[test]
    fn packing_starts_a_new_chunk_at_the_budget_boundary() {
        // Two paragraphs that individually fit but together exceed the budget
        // must land in separate chunks, split on the paragraph boundary.
        let para = "x".repeat(MAX_CHUNK_CHARS - 100);
        let body = format!("{para}\n\n{para}");
        let chunks = chunk_body(&body);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], para);
        assert_eq!(chunks[1], para);
    }

    #[test]
    fn an_oversized_paragraph_is_hard_split_at_whitespace() {
        // A single paragraph well over budget, with spaces, splits into pieces
        // each within budget, and no piece breaks mid-"word".
        let word = "lorem ";
        let para = word.repeat(MAX_CHUNK_CHARS); // way over budget, spaces throughout
        let chunks = chunk_body(&para);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(char_count(chunk) <= MAX_CHUNK_CHARS);
            assert!(!chunk.starts_with(' ') && !chunk.ends_with(' '));
        }
        // Reassembling on single spaces recovers the original words.
        let rejoined = chunks.join(" ");
        assert_eq!(rejoined.split_whitespace().count(), MAX_CHUNK_CHARS);
    }

    #[test]
    fn an_oversized_paragraph_without_whitespace_splits_at_the_char_budget() {
        let para = "a".repeat(MAX_CHUNK_CHARS * 2 + 5);
        let chunks = chunk_body(&para);
        assert_eq!(chunks.len(), 3);
        assert_eq!(char_count(&chunks[0]), MAX_CHUNK_CHARS);
        assert_eq!(char_count(&chunks[1]), MAX_CHUNK_CHARS);
        assert_eq!(char_count(&chunks[2]), 5);
    }

    #[test]
    fn multibyte_text_never_panics_and_stays_within_budget() {
        // Mix CJK and emoji so byte length ≫ char length; a naive byte split
        // would panic on a codepoint boundary.
        let para = "日本語のテキスト🍞".repeat(MAX_CHUNK_CHARS);
        let chunks = chunk_body(&para);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(char_count(chunk) <= MAX_CHUNK_CHARS);
        }
    }

    #[test]
    fn crlf_and_lf_bodies_chunk_identically() {
        let lf = "para one\n\npara two\n\npara three";
        let crlf = "para one\r\n\r\npara two\r\n\r\npara three";
        assert_eq!(chunk_body(lf), chunk_body(crlf));
    }

    #[test]
    fn chunking_is_deterministic() {
        let body = "alpha beta\n\n".repeat(500);
        assert_eq!(chunk_body(&body), chunk_body(&body));
    }

    // --- passage_input ------------------------------------------------------

    #[test]
    fn passage_input_prepends_a_present_title() {
        assert_eq!(
            passage_input("My Title", "the chunk"),
            "My Title\n\nthe chunk"
        );
    }

    #[test]
    fn passage_input_without_a_title_is_the_bare_chunk() {
        assert_eq!(passage_input("", "the chunk"), "the chunk");
    }

    // --- FakeEmbedder -------------------------------------------------------

    #[test]
    fn fake_embedder_is_deterministic_unit_length_and_correct_dim() {
        let fake = FakeEmbedder;
        let a = fake.embed_passages(&["hello world".to_string()]).unwrap();
        let b = fake.embed_passages(&["hello world".to_string()]).unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].len(), EMBEDDING_DIM);
        assert_eq!(fake.dim(), EMBEDDING_DIM);

        let norm = a[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "expected unit length, got {norm}"
        );
    }

    #[test]
    fn fake_embedder_distinguishes_texts_and_matches_query_to_passage() {
        let fake = FakeEmbedder;
        let one = fake.embed_passages(&["one".to_string()]).unwrap();
        let two = fake.embed_passages(&["two".to_string()]).unwrap();
        assert_ne!(one[0], two[0]);

        // A query and the same text as a passage embed identically (the fake
        // applies no prefix), so self-match works.
        let passage = fake.embed_passages(&["needle".to_string()]).unwrap();
        let query = fake.embed_query("needle").unwrap();
        assert_eq!(query, passage[0]);
    }

    // --- index_note pipeline ------------------------------------------------

    /// An [`Embedder`] wrapping [`FakeEmbedder`] that counts passage calls, to
    /// prove when the pipeline embeds versus skips.
    struct CountingEmbedder {
        inner: FakeEmbedder,
        passage_calls: AtomicUsize,
    }

    impl CountingEmbedder {
        fn new() -> Self {
            Self {
                inner: FakeEmbedder,
                passage_calls: AtomicUsize::new(0),
            }
        }
        fn calls(&self) -> usize {
            self.passage_calls.load(Ordering::SeqCst)
        }
    }

    impl Embedder for CountingEmbedder {
        fn dim(&self) -> usize {
            self.inner.dim()
        }
        fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
            self.passage_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.embed_passages(texts)
        }
        fn embed_query(&self, text: &str) -> Result<Vec<f32>, EmbedError> {
            self.inner.embed_query(text)
        }
    }

    fn indexed(id: &str, body: &str) -> IndexedNote {
        IndexedNote {
            id: id.to_string(),
            path: format!("Acme/{id}.md"),
            title: format!("Title {id}"),
            note_type: NoteType::Note,
            project: Some("Acme".to_string()),
            date: "2026-07-11".to_string(),
            tags: vec![],
            source: "manual".to_string(),
            confidence: None,
            category: None,
            category_confidence: None,
            tracking: None,
            body: body.to_string(),
            meeting: None,
        }
    }

    fn chunk_count(index: &NoteIndex, note_id: &str) -> i64 {
        let query = FakeEmbedder.embed_query("anything").unwrap();
        index
            .nearest_chunks(&query, 100)
            .unwrap()
            .iter()
            .filter(|hit| hit.note_id == note_id)
            .count() as i64
    }

    #[test]
    fn index_note_with_an_embedder_stores_one_vector_per_chunk() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let body = format!(
            "{}\n\n{}",
            "x".repeat(MAX_CHUNK_CHARS - 100),
            "y".repeat(200)
        );
        let note = indexed("n_multi", &body);
        let expected = chunk_body(&note.body).len() as i64;
        assert!(expected >= 2, "test needs a multi-chunk body");

        index_note(&mut index, &note, Some(&FakeEmbedder)).unwrap();
        assert_eq!(chunk_count(&index, "n_multi"), expected);
    }

    #[test]
    fn index_note_without_an_embedder_upserts_but_stores_no_vectors() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let note = indexed("n_novec", "some body");
        index_note(&mut index, &note, None).unwrap();

        assert!(index.get_note("n_novec").unwrap().is_some());
        assert!(!index.note_has_chunks("n_novec").unwrap());
    }

    #[test]
    fn re_indexing_unchanged_content_does_not_re_embed() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let embedder = CountingEmbedder::new();
        let note = indexed("n_same", "stable body text");

        index_note(&mut index, &note, Some(&embedder)).unwrap();
        assert_eq!(embedder.calls(), 1);

        // Same content again → the upsert keeps the chunks → no re-embed.
        index_note(&mut index, &note, Some(&embedder)).unwrap();
        assert_eq!(embedder.calls(), 1, "unchanged content must not re-embed");
    }

    #[test]
    fn editing_the_body_re_embeds_and_replaces_the_chunks() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let embedder = CountingEmbedder::new();
        let mut note = indexed("n_edit", "original body");
        index_note(&mut index, &note, Some(&embedder)).unwrap();
        assert_eq!(embedder.calls(), 1);
        assert_eq!(chunk_count(&index, "n_edit"), 1);

        note.body = "a completely rewritten body".to_string();
        index_note(&mut index, &note, Some(&embedder)).unwrap();
        assert_eq!(embedder.calls(), 2, "an edit must re-embed");
        assert_eq!(chunk_count(&index, "n_edit"), 1);

        // The stored chunk text reflects the new body.
        let query = FakeEmbedder.embed_query("x").unwrap();
        let hits = index.nearest_chunks(&query, 10).unwrap();
        assert!(hits.iter().any(|h| h.note_id == "n_edit"));
    }

    #[test]
    fn an_empty_body_note_is_upserted_but_has_no_chunks() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let note = indexed("n_empty", "   ");
        index_note(&mut index, &note, Some(&FakeEmbedder)).unwrap();
        assert!(index.get_note("n_empty").unwrap().is_some());
        assert!(!index.note_has_chunks("n_empty").unwrap());
    }

    // --- composable seams (upsert_and_plan / plan_from_content / store) -------

    #[test]
    fn plan_from_content_prefixes_the_title_and_skips_empty_bodies() {
        assert_eq!(plan_from_content("Title", "  \n\n  "), None);

        let plan = plan_from_content("Title", "one paragraph").unwrap();
        assert_eq!(plan.chunks, vec!["one paragraph".to_string()]);
        assert_eq!(plan.inputs, vec!["Title\n\none paragraph".to_string()]);
    }

    #[test]
    fn upsert_and_plan_re_plans_only_when_content_changes() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let note = indexed("n_plan", "stable body");

        // First time through there are no chunk rows, so it plans an embed.
        let plan = upsert_and_plan(&mut index, &note).unwrap().unwrap();
        store_embeddings(
            &mut index,
            &note.id,
            &plan.chunks,
            FakeEmbedder.embed_passages(&plan.inputs).unwrap(),
        )
        .unwrap();

        // Unchanged content keeps the chunk rows, so nothing is planned.
        assert_eq!(upsert_and_plan(&mut index, &note).unwrap(), None);

        // Editing the body invalidates the rows, so a fresh plan appears.
        let mut edited = note.clone();
        edited.body = "a different body".to_string();
        assert!(upsert_and_plan(&mut index, &edited).unwrap().is_some());
    }

    #[test]
    fn store_embeddings_rejects_a_vector_count_mismatch() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let chunks = vec!["a".to_string(), "b".to_string()];
        // One vector for two chunks — a broken embedder contract.
        let embeddings = FakeEmbedder.embed_passages(&["a".to_string()]).unwrap();
        let err = store_embeddings(&mut index, "n_mismatch", &chunks, embeddings);
        assert!(matches!(
            err,
            Err(PipelineError::Embed(EmbedError::Backend(_)))
        ));
        // Nothing was written.
        assert!(!index.note_has_chunks("n_mismatch").unwrap());
    }
}
