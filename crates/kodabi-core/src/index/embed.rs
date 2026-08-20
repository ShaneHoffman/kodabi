//! The vector-store side of the index: writing a note's chunk embeddings and
//! querying them by nearest neighbour.
//!
//! `upsert_note` (see `query.rs`) owns *invalidating* a note's vectors when its
//! content changes; this module owns *writing* the fresh ones and reading them
//! back. Vectors live in the `notes_vec` `vec0` table (one row per body chunk,
//! keyed by a synthetic `chunk_id`), with each chunk's source text mirrored in
//! `note_chunks` so a search hit can carry a snippet without re-reading the file.
//! The two tables are kept in lockstep: a `note_chunks` row exists iff its
//! `notes_vec` row does.

use rusqlite::{params, OptionalExtension};

use super::{IndexError, NoteIndex, Result, EMBEDDING_DIM};

/// One embedded chunk of a note's body: the exact text that was embedded and
/// its vector. The vector must be [`EMBEDDING_DIM`] long (validated on write)
/// and is expected to be L2-normalized by the embedder.
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedChunk {
    pub text: String,
    pub embedding: Vec<f32>,
}

/// A nearest-neighbour hit from [`NoteIndex::nearest_chunks`]: which note and
/// chunk matched, and the `vec0` distance (smaller is nearer). Because stored
/// vectors are L2-normalized, this L2 distance rank-orders identically to
/// cosine similarity.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkHit {
    pub note_id: String,
    pub seq: i64,
    pub distance: f64,
}

/// Encodes a vector as the little-endian `f32` byte blob `sqlite-vec` reads as a
/// `FLOAT[N]` value — the compact binding, versus a JSON array text literal.
///
/// Shared with `super::search`, whose vector arm binds the query embedding the
/// same way [`nearest_chunks`](NoteIndex::nearest_chunks) binds its query.
pub(super) fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(embedding.len() * 4);
    for value in embedding {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

impl NoteIndex {
    /// Replaces *all* of `note_id`'s chunk rows — in both `notes_vec` and
    /// `note_chunks` — with `chunks`, in one transaction. Passing an empty slice
    /// clears the note's chunks. Every embedding must be [`EMBEDDING_DIM`] long;
    /// a mismatch aborts the whole write with [`IndexError::EmbeddingDim`].
    ///
    /// Chunks are keyed by `seq` (their index in the slice), so re-writing with
    /// fewer chunks drops the surplus rows. This is the pipeline's sole entry to
    /// the vector store; `upsert_note` only ever deletes.
    pub fn set_note_chunks(&mut self, note_id: &str, chunks: &[EmbeddedChunk]) -> Result<()> {
        for chunk in chunks {
            if chunk.embedding.len() != EMBEDDING_DIM {
                return Err(IndexError::EmbeddingDim {
                    expected: EMBEDDING_DIM,
                    got: chunk.embedding.len(),
                });
            }
        }

        let tx = self.conn.transaction()?;
        // Clear the old rows wholesale so a shorter re-chunk leaves no orphans;
        // `note_id` is a `vec0` metadata column, so the vector delete needs no
        // key parsing.
        tx.execute("DELETE FROM notes_vec WHERE note_id = ?1", [note_id])?;
        tx.execute("DELETE FROM note_chunks WHERE note_id = ?1", [note_id])?;
        {
            let mut insert_vec = tx.prepare(
                "INSERT INTO notes_vec (chunk_id, note_id, seq, embedding)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut insert_text =
                tx.prepare("INSERT INTO note_chunks (note_id, seq, text) VALUES (?1, ?2, ?3)")?;
            for (seq, chunk) in chunks.iter().enumerate() {
                let seq = seq as i64;
                let chunk_id = format!("{note_id}#{seq:04}");
                insert_vec.execute(params![
                    chunk_id,
                    note_id,
                    seq,
                    embedding_to_blob(&chunk.embedding),
                ])?;
                insert_text.execute(params![note_id, seq, chunk.text])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Whether `note_id` has any chunk rows — the pipeline's "already embedded?"
    /// check that lets a re-index of unchanged content skip re-embedding.
    pub fn note_has_chunks(&self, note_id: &str) -> Result<bool> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT 1 FROM note_chunks WHERE note_id = ?1 LIMIT 1",
                [note_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// The ids of indexed notes that need embeddings computed — a non-empty
    /// body but no chunk rows yet. This is the reconcile embed sweep's work-list
    /// (after a watcher burst or rebuild). Ordered by id for deterministic
    /// sweeps.
    ///
    /// Notes whose body is empty or all whitespace are *excluded*: the chunker
    /// ([`crate::embed::chunk_body`]) yields no chunks for them, so they
    /// legitimately have zero chunk rows and never need embedding. Excluding
    /// them keeps a redundant sweep from re-reading every title-only note on
    /// every pass — otherwise each such note would reappear here forever and the
    /// sweep would lock the index and re-read it on every reconcile. The
    /// whitespace set (`trim`'s second argument) is the ASCII whitespace the
    /// chunker's `str::trim` collapses; an exotic-Unicode-only body is a
    /// harmless, vanishingly rare miss (it just no-ops in the sweep as before).
    pub fn note_ids_missing_embeddings(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM notes
             WHERE id NOT IN (SELECT DISTINCT note_id FROM note_chunks)
               AND trim(body, ' ' || char(9) || char(10) || char(11) || char(12) || char(13)) != ''
             ORDER BY id",
        )?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }

    /// The `k` chunk vectors nearest to `query`, nearest first. The search
    /// surface (#50) builds its vector arm on this shape; it exists here to
    /// prove the store round-trips end to end.
    pub fn nearest_chunks(&self, query: &[f32], k: usize) -> Result<Vec<ChunkHit>> {
        let mut stmt = self.conn.prepare(
            "SELECT note_id, seq, distance FROM notes_vec
             WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
        )?;
        let hits = stmt
            .query_map(params![embedding_to_blob(query), k as i64], |row| {
                Ok(ChunkHit {
                    note_id: row.get(0)?,
                    seq: row.get(1)?,
                    distance: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<ChunkHit>>>()?;
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{IndexedNote, NoteType};

    fn indexed(id: &str) -> IndexedNote {
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
            body: format!("body of {id}"),
            meeting: None,
        }
    }

    /// A unit vector whose only nonzero component is at `axis` — orthogonal
    /// vectors for different axes, so nearest-neighbour ordering is predictable.
    fn axis(axis: usize) -> Vec<f32> {
        let mut v = vec![0.0; EMBEDDING_DIM];
        v[axis] = 1.0;
        v
    }

    fn note_id_chunk_counts(index: &NoteIndex, note_id: &str) -> (i64, i64) {
        let vecs = index
            .conn
            .query_row(
                "SELECT count(*) FROM notes_vec WHERE note_id = ?1",
                [note_id],
                |r| r.get(0),
            )
            .unwrap();
        let texts = index
            .conn
            .query_row(
                "SELECT count(*) FROM note_chunks WHERE note_id = ?1",
                [note_id],
                |r| r.get(0),
            )
            .unwrap();
        (vecs, texts)
    }

    #[test]
    fn set_note_chunks_populates_both_tables_in_lockstep() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let chunks = vec![
            EmbeddedChunk {
                text: "alpha".to_string(),
                embedding: axis(0),
            },
            EmbeddedChunk {
                text: "beta".to_string(),
                embedding: axis(1),
            },
        ];
        index.set_note_chunks("n_two", &chunks).unwrap();
        assert_eq!(note_id_chunk_counts(&index, "n_two"), (2, 2));

        // The stored text is exactly what was embedded, addressable by seq.
        let text: String = index
            .conn
            .query_row(
                "SELECT text FROM note_chunks WHERE note_id = 'n_two' AND seq = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(text, "beta");
    }

    #[test]
    fn re_writing_with_fewer_chunks_removes_the_surplus() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let three = vec![
            EmbeddedChunk {
                text: "a".to_string(),
                embedding: axis(0),
            },
            EmbeddedChunk {
                text: "b".to_string(),
                embedding: axis(1),
            },
            EmbeddedChunk {
                text: "c".to_string(),
                embedding: axis(2),
            },
        ];
        index.set_note_chunks("n_shrink", &three).unwrap();
        assert_eq!(note_id_chunk_counts(&index, "n_shrink"), (3, 3));

        index.set_note_chunks("n_shrink", &three[..1]).unwrap();
        assert_eq!(note_id_chunk_counts(&index, "n_shrink"), (1, 1));

        // An empty slice clears the note entirely.
        index.set_note_chunks("n_shrink", &[]).unwrap();
        assert_eq!(note_id_chunk_counts(&index, "n_shrink"), (0, 0));
    }

    #[test]
    fn a_wrong_length_embedding_is_rejected_and_writes_nothing() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let bad = vec![EmbeddedChunk {
            text: "x".to_string(),
            embedding: vec![0.0; EMBEDDING_DIM - 1],
        }];
        assert!(matches!(
            index.set_note_chunks("n_bad", &bad),
            Err(IndexError::EmbeddingDim {
                expected: EMBEDDING_DIM,
                got,
            }) if got == EMBEDDING_DIM - 1
        ));
        assert_eq!(note_id_chunk_counts(&index, "n_bad"), (0, 0));
    }

    #[test]
    fn a_blob_bound_vector_is_matchable_by_a_json_literal_query() {
        // The store binds vectors as little-endian f32 blobs; prove that is
        // byte-compatible with sqlite-vec by matching one with a JSON-array
        // text literal (the form the crate's own tests use).
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .set_note_chunks(
                "n_blob",
                &[EmbeddedChunk {
                    text: "only".to_string(),
                    embedding: axis(0),
                }],
            )
            .unwrap();

        let json = {
            let mut parts = vec!["1".to_string()];
            parts.extend(std::iter::repeat_n("0".to_string(), EMBEDDING_DIM - 1));
            format!("[{}]", parts.join(","))
        };
        let hit: String = index
            .conn
            .query_row(
                "SELECT note_id FROM notes_vec
                 WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                [json],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hit, "n_blob");
    }

    #[test]
    fn note_has_chunks_tracks_writes() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        assert!(!index.note_has_chunks("n_x").unwrap());
        index
            .set_note_chunks(
                "n_x",
                &[EmbeddedChunk {
                    text: "t".to_string(),
                    embedding: axis(0),
                }],
            )
            .unwrap();
        assert!(index.note_has_chunks("n_x").unwrap());
        index.set_note_chunks("n_x", &[]).unwrap();
        assert!(!index.note_has_chunks("n_x").unwrap());
    }

    #[test]
    fn missing_embeddings_lists_only_unembedded_notes_in_order() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index.upsert_note(&indexed("n_aaa")).unwrap();
        index.upsert_note(&indexed("n_bbb")).unwrap();

        // Both start unembedded.
        assert_eq!(
            index.note_ids_missing_embeddings().unwrap(),
            vec!["n_aaa".to_string(), "n_bbb".to_string()]
        );

        index
            .set_note_chunks(
                "n_aaa",
                &[EmbeddedChunk {
                    text: "t".to_string(),
                    embedding: axis(0),
                }],
            )
            .unwrap();
        assert_eq!(
            index.note_ids_missing_embeddings().unwrap(),
            vec!["n_bbb".to_string()]
        );
    }

    #[test]
    fn missing_embeddings_excludes_empty_body_notes() {
        // A title-only note chunks to nothing, so it never needs embedding and
        // must not linger in the work-list (else every reconcile re-reads it).
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut has_body = indexed("n_full0");
        has_body.body = "some real content".to_string();
        let mut empty = indexed("n_bare0");
        empty.body = String::new();
        let mut whitespace = indexed("n_ws000");
        whitespace.body = "  \n\n\t ".to_string();
        for note in [&has_body, &empty, &whitespace] {
            index.upsert_note(note).unwrap();
        }

        // Only the note with real content appears; the empty/whitespace ones,
        // which the chunker yields nothing for, are excluded.
        assert_eq!(
            index.note_ids_missing_embeddings().unwrap(),
            vec!["n_full0".to_string()]
        );
    }

    #[test]
    fn nearest_chunks_orders_by_distance_and_respects_k() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .set_note_chunks(
                "n_near",
                &[EmbeddedChunk {
                    text: "near".to_string(),
                    embedding: axis(0),
                }],
            )
            .unwrap();
        index
            .set_note_chunks(
                "n_far",
                &[EmbeddedChunk {
                    text: "far".to_string(),
                    embedding: axis(1),
                }],
            )
            .unwrap();

        // Querying along axis 0 puts n_near first.
        let hits = index.nearest_chunks(&axis(0), 2).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].note_id, "n_near");
        assert!(hits[0].distance <= hits[1].distance);

        // k caps the result count.
        let one = index.nearest_chunks(&axis(0), 1).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].note_id, "n_near");
    }
}
