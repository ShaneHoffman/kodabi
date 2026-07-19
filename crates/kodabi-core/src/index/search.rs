//! Hybrid full-text + semantic retrieval, exposed as `search_notes`.
//!
//! Runs the query against both arms of the index — FTS5 (`notes_fts`) and the
//! `sqlite-vec` vectors (`notes_vec`) — and merges the two ranked lists with
//! **Reciprocal Rank Fusion** (RRF). The shape mirrors the `search_notes`
//! contract in `docs/MCP_TOOL_SURFACE.md` exactly: [`SearchParams`] is the
//! `inputSchema`, [`SearchResults`] (`hits` + `page`) the `outputSchema`, and
//! [`SearchHit`] is a `NoteSummary` augmented with `score`/`rank`/`snippet`.
//! This is pure `kodabi-core`; Phase 3's MCP server deserializes tool arguments
//! straight into [`SearchParams`] and serializes [`SearchResults`] back out.
//!
//! ## Fusion
//!
//! Each arm produces up to a bounded pool of candidates in its own ranking
//! (FTS by `bm25` ascending, vectors by distance ascending, deduped to the
//! nearest chunk per note). A note's fused score is `Σ 1 / (RRF_K + rank)` over
//! the arms that surfaced it, so appearing high in *either* arm helps and
//! appearing in both helps most. The fused list is ordered by score descending,
//! ties broken by `id` ascending, giving a total order that pagination cursors
//! walk deterministically.
//!
//! ## Degradation
//!
//! The default app build ships without an embedder (the `bge` backend is
//! feature-gated), so FTS-only is a first-class service level, not an edge
//! case. A missing embedder, an `embed_query` failure, or a mis-dimensioned
//! query vector all drop the vector arm silently and the search proceeds on
//! FTS alone — the output shape has no channel to report partial degradation,
//! and returning *some* results beats erroring.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, OptionalExtension};

use super::embed::embedding_to_blob;
use super::note::{NoteRow, NoteType};
use super::query::{map_row, NOTE_COLUMNS};
use super::{IndexError, NoteIndex, Result, EMBEDDING_DIM};
use crate::embed::Embedder;

/// The RRF constant. 60 is the value from the original Cormack et al. RRF paper
/// and the de-facto default; the contract leaves it to the implementer. Larger
/// values flatten the contribution of top ranks (less weight on being #1).
const RRF_K: f64 = 60.0;

/// How many FTS hits enter fusion, by `bm25` order. A chatty query with a
/// common term can match a lot of notes; the cap keeps fusion bounded while
/// still covering far more than any one page.
const FTS_CANDIDATES: i64 = 200;

/// How many *chunk* vectors the KNN over-fetches before per-note dedup and
/// filtering. Over-fetched because a note can own several chunks and because
/// post-filtering shrinks the pool: v1 uses a fixed over-fetch rather than
/// iterative widening, so under a highly selective filter only *semantic-only*
/// recall degrades — the FTS arm still filters over the whole corpus in SQL.
const VEC_CHUNK_CANDIDATES: i64 = 400;

/// How many notes the vector arm contributes after deduping to the nearest
/// chunk per note.
const VEC_NOTE_CANDIDATES: usize = 200;

/// Cap on query tokens fed to the FTS arm — a guard against a pathological
/// multi-kilobyte query, not a limit real queries hit.
const MAX_QUERY_TOKENS: usize = 64;

/// Character budget for a vector-only hit's snippet (the note's own chunk text).
/// FTS hits get their snippet from `snippet()` instead.
const SNIPPET_MAX_CHARS: usize = 320;

/// Contract bounds on `limit` (`minimum`/`maximum`/`default` in the schema).
const MIN_LIMIT: u32 = 1;
const MAX_LIMIT: u32 = 50;
const DEFAULT_LIMIT: u32 = 10;

fn default_true() -> bool {
    true
}

fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

/// Whether a tag filter requires *any* or *all* of the listed tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagMatch {
    /// A hit must carry at least one of the listed tags (the contract default).
    #[default]
    Any,
    /// A hit must carry every listed tag.
    All,
}

/// The `search_notes` inputs — mirrors the tool's `inputSchema` field-for-field,
/// including its defaults, so an MCP wrapper can deserialize tool arguments
/// straight into it. `deny_unknown_fields` matches the schema's
/// `additionalProperties: false`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Natural-language or keyword query, fed to both arms. Required.
    pub query: String,
    /// Restrict to this project (a hierarchical slug). The reserved value
    /// `Inbox` (any casing) matches unfiled notes.
    #[serde(default)]
    pub project: Option<String>,
    /// When `project` is set, also match its sub-projects. Default `true`.
    #[serde(default = "default_true")]
    pub include_descendants: bool,
    /// Restrict to these note types; empty means all. Named `type` on the wire.
    #[serde(rename = "type", default)]
    pub types: Vec<NoteType>,
    /// Restrict to notes carrying these tags (see [`tag_match`](Self::tag_match)).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether a hit needs any or all of `tags`. Default [`TagMatch::Any`].
    #[serde(default)]
    pub tag_match: TagMatch,
    /// Inclusive lower bound on the note date (`YYYY-MM-DD`).
    #[serde(default)]
    pub date_from: Option<String>,
    /// Inclusive upper bound on the note date (`YYYY-MM-DD`).
    #[serde(default)]
    pub date_to: Option<String>,
    /// Max hits in this page, clamped to `1..=50`. Default `10`.
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Opaque pagination token from a prior response's `page.next_cursor`.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// A ranked search hit: a `NoteSummary` augmented with retrieval `score`,
/// `rank`, and `snippet`. Field names and casing match the `SearchHit`/
/// `NoteSummary` `$defs` in `docs/MCP_TOOL_SURFACE.md`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchHit {
    pub id: String,
    pub path: String,
    pub title: String,
    #[serde(rename = "type")]
    pub note_type: NoteType,
    /// Owning project slug, or `null` when unfiled (Inbox).
    pub project: Option<String>,
    /// Frontmatter `date`, verbatim as stored (offset preserved).
    pub date: String,
    pub tags: Vec<String>,
    pub source: String,
    /// Routing confidence 0..1, or `null` when none exists.
    pub confidence: Option<f64>,
    /// Fused RRF relevance score; higher is better.
    pub score: f64,
    /// 1-based rank within the full fused result set.
    pub rank: u32,
    /// Highlighted excerpt of the matching passage.
    pub snippet: String,
}

/// Cursor-based pagination envelope. Cursors are opaque and mutation-safe: they
/// encode the fused sort key + id of the last hit, not an offset, so the index
/// changing under the watcher between pages never skips or duplicates rows at
/// the page boundary.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PageInfo {
    /// True if more results exist beyond this page.
    pub has_more: bool,
    /// Token to pass as the next request's `cursor`; `null` when `has_more` is
    /// false.
    pub next_cursor: Option<String>,
    /// Approximate total matches (the fused candidate count — a lower bound,
    /// since the candidate pools are capped), or `null` when not known.
    pub total_estimate: Option<u64>,
}

/// The `search_notes` output — ranked `hits` plus the pagination `page`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub page: PageInfo,
}

/// A decoded pagination cursor: the fused sort key of the last hit on the prior
/// page.
struct CursorKey {
    score: f64,
    id: String,
}

/// A shared filter fragment (`AND`-joined SQL clauses) plus its bound values,
/// applied identically to both arms so a filter narrows the semantic search the
/// same way it narrows full-text search.
struct Filters {
    clauses: Vec<String>,
    values: Vec<Value>,
}

impl Filters {
    /// The clauses as ` AND (…)` suffixes to splice after an arm's own `WHERE`
    /// predicate, or an empty string when there are no filters.
    fn where_and(&self) -> String {
        self.clauses.iter().map(|c| format!(" AND {c}")).collect()
    }
}

impl NoteIndex {
    /// Hybrid FTS5 + `sqlite-vec` search fused with RRF, matching the
    /// `search_notes` contract.
    ///
    /// `embedder` powers the semantic arm; pass `None` (or a broken embedder)
    /// to search FTS-only. A whitespace-only `query` returns an empty page
    /// (absence is a valid answer, per the contract). `limit` is clamped to
    /// `1..=50`. A malformed `cursor` is an [`IndexError::Cursor`]; a malformed
    /// `date_from`/`date_to` is an [`IndexError::Date`].
    pub fn search_notes(
        &self,
        params: &SearchParams,
        embedder: Option<&dyn Embedder>,
    ) -> Result<SearchResults> {
        let limit = params.limit.clamp(MIN_LIMIT, MAX_LIMIT) as usize;

        // A query with nothing to match on: succeed with an empty page rather
        // than error — absence is a valid answer (MCP_TOOL_SURFACE.md §2).
        if params.query.trim().is_empty() {
            return Ok(empty_results());
        }

        let filters = build_filters(params)?;
        let cursor = params.cursor.as_deref().map(decode_cursor).transpose()?;

        // Arm 1: full-text. Skipped when sanitizing leaves no usable term, so no
        // invalid FTS5 MATCH expression ever reaches SQLite.
        let fts = match sanitize_fts_query(&params.query) {
            Some(expr) => self.fts_arm(&expr, &filters)?,
            None => Vec::new(),
        };

        // Arm 2: semantic. Degrades to empty on any embedding problem.
        let vector = self.vector_candidates(&params.query, embedder, &filters)?;

        // Fuse via RRF: a note's score sums 1/(k+rank) over the arms it appears
        // in, so both-arms notes rise above single-arm ones.
        let mut scores: HashMap<String, f64> = HashMap::new();
        for (rank0, (id, _)) in fts.iter().enumerate() {
            *scores.entry(id.clone()).or_insert(0.0) += rrf_term(rank0);
        }
        for (rank0, (id, _)) in vector.iter().enumerate() {
            *scores.entry(id.clone()).or_insert(0.0) += rrf_term(rank0);
        }

        // Total order: score descending, ties by id ascending — the id tiebreak
        // is load-bearing because equal RRF scores are common and pagination
        // needs a deterministic order.
        let mut fused: Vec<(String, f64)> = scores.into_iter().collect();
        fused.sort_by(fused_order);
        let total = fused.len();

        // Keyset pagination: resume strictly after the cursor's fused key.
        let start = match &cursor {
            Some(key) => {
                let cursor_item = (key.id.clone(), key.score);
                fused.partition_point(|item| fused_order(item, &cursor_item) != Ordering::Greater)
            }
            None => 0,
        };
        let end = (start + limit).min(total);
        let page = &fused[start..end];

        // Hydrate only the page: one row fetch + one batched tag load.
        let page_ids: Vec<&str> = page.iter().map(|(id, _)| id.as_str()).collect();
        let mut rows = self.load_note_rows(&page_ids)?;
        let fts_snippets: HashMap<&str, &str> = fts
            .iter()
            .map(|(id, s)| (id.as_str(), s.as_str()))
            .collect();
        let vec_seq: HashMap<&str, i64> =
            vector.iter().map(|(id, seq)| (id.as_str(), *seq)).collect();

        let mut hits = Vec::with_capacity(page.len());
        for (offset, (id, score)) in page.iter().enumerate() {
            // A row can vanish if a concurrent writer deletes it between the arm
            // query and hydration — drop it from the page rather than fail.
            let Some(row) = rows.remove(id) else {
                continue;
            };
            let snippet = match fts_snippets.get(id.as_str()) {
                // FTS snippet is query-aware and highlighted — prefer it.
                Some(snippet) => (*snippet).to_string(),
                // Vector-only hit: the nearest chunk's own words, truncated.
                None => {
                    let seq = vec_seq.get(id.as_str()).copied().unwrap_or(0);
                    self.chunk_text(id, seq)?
                        .map(|text| truncate_snippet(&text))
                        .unwrap_or_default()
                }
            };
            hits.push(SearchHit {
                id: row.id,
                path: row.path,
                title: row.title,
                note_type: row.note_type,
                project: row.project,
                date: row.date,
                tags: row.tags,
                source: row.source,
                confidence: row.confidence,
                score: *score,
                rank: (start + offset + 1) as u32,
                snippet,
            });
        }

        let has_more = end < total;
        // Encode from the last *fused* position consumed (not the last hydrated
        // hit) so a concurrently-deleted boundary row still advances the cursor.
        let next_cursor = has_more.then(|| {
            let (id, score) = &fused[end - 1];
            encode_cursor(*score, id)
        });

        Ok(SearchResults {
            hits,
            page: PageInfo {
                has_more,
                next_cursor,
                total_estimate: Some(total as u64),
            },
        })
    }

    /// Runs the FTS arm: notes matching `match_expr` (under `filters`), best
    /// `bm25` first, each with a highlighted snippet. Returns `(id, snippet)`
    /// in rank order, capped at [`FTS_CANDIDATES`].
    fn fts_arm(&self, match_expr: &str, filters: &Filters) -> Result<Vec<(String, String)>> {
        let sql = format!(
            "SELECT notes.id, snippet(notes_fts, -1, '**', '**', '…', 16)
             FROM notes_fts JOIN notes ON notes.pk = notes_fts.rowid
             WHERE notes_fts MATCH ?{filters}
             ORDER BY bm25(notes_fts) LIMIT ?",
            filters = filters.where_and()
        );

        // Bind order follows the placeholders: MATCH expr, filter values, LIMIT.
        let mut values = Vec::with_capacity(filters.values.len() + 2);
        values.push(Value::Text(match_expr.to_string()));
        values.extend(filters.values.iter().cloned());
        values.push(Value::Integer(FTS_CANDIDATES));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        Ok(rows)
    }

    /// The vector arm's contribution, or an empty list when the semantic search
    /// can't run: no embedder, an `embed_query` failure (a broken model), or a
    /// mis-dimensioned vector (guarded here so it can't surface as an opaque
    /// `sqlite-vec` bind error). Any of these degrades the search to FTS-only.
    fn vector_candidates(
        &self,
        query: &str,
        embedder: Option<&dyn Embedder>,
        filters: &Filters,
    ) -> Result<Vec<(String, i64)>> {
        let Some(embedder) = embedder else {
            return Ok(Vec::new());
        };
        let Ok(query_vec) = embedder.embed_query(query) else {
            return Ok(Vec::new());
        };
        if query_vec.len() != EMBEDDING_DIM {
            return Ok(Vec::new());
        }
        self.vector_arm(&query_vec, filters)
    }

    /// Runs the KNN vector arm: the chunks nearest `query_vec` (under
    /// `filters`), deduped to the nearest chunk per note. Returns `(note_id,
    /// seq)` in distance order, capped at [`VEC_NOTE_CANDIDATES`]. The `seq` is
    /// the nearest chunk's index, used to source the note's snippet.
    fn vector_arm(&self, query_vec: &[f32], filters: &Filters) -> Result<Vec<(String, i64)>> {
        // The KNN LIMIT must live inside the subquery (the `vec0` constraint);
        // the join + filter run in the outer query.
        let sql = format!(
            "SELECT v.note_id, v.seq
             FROM (SELECT note_id, seq, distance FROM notes_vec
                   WHERE embedding MATCH ? ORDER BY distance LIMIT ?) AS v
             JOIN notes ON notes.id = v.note_id
             WHERE 1 = 1{filters}
             ORDER BY v.distance",
            filters = filters.where_and()
        );

        // Bind order: embedding blob, KNN LIMIT, then the outer filter values.
        let mut values = Vec::with_capacity(filters.values.len() + 2);
        values.push(Value::Blob(embedding_to_blob(query_vec)));
        values.push(Value::Integer(VEC_CHUNK_CANDIDATES));
        values.extend(filters.values.iter().cloned());

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(values), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;

        // Rows arrive nearest-first, so the first sighting of a note is its
        // nearest chunk. Keep that, drop the rest.
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for row in rows {
            let (note_id, seq) = row?;
            if seen.insert(note_id.clone()) {
                out.push((note_id, seq));
                if out.len() == VEC_NOTE_CANDIDATES {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Loads the [`NoteRow`]s for a page of ids (with tags), keyed by id. Ids
    /// absent from the index — deleted since the arm queries ran — simply don't
    /// appear in the map.
    fn load_note_rows(&self, ids: &[&str]) -> Result<HashMap<String, NoteRow>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id IN ({placeholders})");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(ids.iter()), map_row)?
            .collect::<rusqlite::Result<Vec<NoteRow>>>()?;

        let mut by_id: HashMap<String, NoteRow> =
            rows.into_iter().map(|row| (row.id.clone(), row)).collect();
        let present: Vec<&str> = by_id.keys().map(String::as_str).collect();
        let mut tags = self.load_tags_by_ids(&present)?;
        for (id, row) in &mut by_id {
            row.tags = tags.remove(id).unwrap_or_default();
        }
        Ok(by_id)
    }

    /// The stored text of one chunk, or `None` if it isn't present (a note
    /// re-chunked between the vector query and this lookup).
    fn chunk_text(&self, note_id: &str, seq: i64) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT text FROM note_chunks WHERE note_id = ?1 AND seq = ?2")?;
        let text = stmt
            .query_row(params![note_id, seq], |row| row.get::<_, String>(0))
            .optional()?;
        Ok(text)
    }
}

/// One arm's RRF contribution for a 0-based rank: `1 / (k + rank)`.
fn rrf_term(rank0: usize) -> f64 {
    1.0 / (RRF_K + (rank0 + 1) as f64)
}

/// The fused total order: score descending, ties broken by id ascending.
/// `total_cmp` gives a deterministic total order over the f64 scores (no NaN is
/// produced, but this keeps the sort total and clippy-clean).
fn fused_order(a: &(String, f64), b: &(String, f64)) -> Ordering {
    b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0))
}

/// An empty successful page — the answer for a no-op query or no matches.
fn empty_results() -> SearchResults {
    SearchResults {
        hits: Vec::new(),
        page: PageInfo {
            has_more: false,
            next_cursor: None,
            total_estimate: Some(0),
        },
    }
}

/// Turns raw query text into a safe FTS5 MATCH expression, or `None` when no
/// usable term survives (an all-punctuation query).
///
/// Every whitespace token that carries at least one alphanumeric char is wrapped
/// as an FTS5 string literal (doubling any embedded `"`), and the literals are
/// joined with `OR`. Wrapping means no token content — `title:`, `NEAR(`, `*`,
/// `(` — is ever parsed as MATCH syntax, so a raw natural-language query can't
/// raise a syntax error. `OR` (not `AND`) because unicode61 removes no
/// stopwords: an `AND` of a chatty query would require every word and match
/// nothing, while `OR` keeps recall and lets `bm25` rank the multi-term matches
/// highest.
fn sanitize_fts_query(query: &str) -> Option<String> {
    let mut terms = Vec::new();
    for token in query.split_whitespace() {
        if !token.chars().any(char::is_alphanumeric) {
            continue;
        }
        let escaped = token.replace('"', "\"\"");
        terms.push(format!("\"{escaped}\""));
        if terms.len() == MAX_QUERY_TOKENS {
            break;
        }
    }
    (!terms.is_empty()).then(|| terms.join(" OR "))
}

/// Builds the shared filter clauses + bound values from the search params.
fn build_filters(params: &SearchParams) -> Result<Filters> {
    let mut clauses = Vec::new();
    let mut values = Vec::new();

    if let Some(project) = &params.project {
        if project.eq_ignore_ascii_case("Inbox") {
            // The reserved Inbox sentinel maps to an unfiled note (project NULL).
            clauses.push("notes.project IS NULL".to_string());
        } else if params.include_descendants {
            // Exact match, or any descendant under `project/`. The LIKE pattern
            // escapes the slug's own `%`/`_`/`\` so they stay literal.
            clauses.push("(notes.project = ? OR notes.project LIKE ? ESCAPE '\\')".to_string());
            values.push(Value::Text(project.clone()));
            values.push(Value::Text(format!("{}/%", like_escape(project))));
        } else {
            clauses.push("notes.project = ?".to_string());
            values.push(Value::Text(project.clone()));
        }
    }

    if !params.types.is_empty() {
        let placeholders = sql_placeholders(params.types.len());
        clauses.push(format!("notes.type IN ({placeholders})"));
        for note_type in &params.types {
            values.push(Value::Text(note_type.as_str().to_string()));
        }
    }

    if !params.tags.is_empty() {
        let placeholders = sql_placeholders(params.tags.len());
        match params.tag_match {
            TagMatch::Any => {
                clauses.push(format!(
                    "EXISTS (SELECT 1 FROM note_tags
                     WHERE note_tags.note_pk = notes.pk AND note_tags.tag IN ({placeholders}))"
                ));
                for tag in &params.tags {
                    values.push(Value::Text(tag.clone()));
                }
            }
            TagMatch::All => {
                clauses.push(format!(
                    "(SELECT count(DISTINCT note_tags.tag) FROM note_tags
                      WHERE note_tags.note_pk = notes.pk AND note_tags.tag IN ({placeholders}))
                     = ?"
                ));
                for tag in &params.tags {
                    values.push(Value::Text(tag.clone()));
                }
                values.push(Value::Integer(params.tags.len() as i64));
            }
        }
    }

    // Date bounds compare against `date_utc` (always `…T..:..:..Z`), so the
    // filter is by absolute instant, not wall clock. Inclusive both ends.
    if let Some(from) = &params.date_from {
        let day = parse_iso_date(from)?;
        clauses.push("notes.date_utc >= ?".to_string());
        values.push(Value::Text(format!("{day}T00:00:00Z")));
    }
    if let Some(to) = &params.date_to {
        let day = parse_iso_date(to)?;
        clauses.push("notes.date_utc <= ?".to_string());
        values.push(Value::Text(format!("{day}T23:59:59Z")));
    }

    Ok(Filters { clauses, values })
}

/// `?,?,…` for `n` bound values.
fn sql_placeholders(n: usize) -> String {
    std::iter::repeat_n("?", n).collect::<Vec<_>>().join(",")
}

/// Escapes a string for use inside a `LIKE … ESCAPE '\'` pattern so its own
/// `%`, `_`, and `\` stay literal.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Validates an `IsoDate` bound and returns its canonical `YYYY-MM-DD` form.
fn parse_iso_date(raw: &str) -> Result<String> {
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .map(|date| date.format("%Y-%m-%d").to_string())
        .map_err(|source| IndexError::Date {
            value: raw.to_string(),
            source,
        })
}

/// Encodes a fused sort key (`score` bits + `id`) as an opaque cursor token.
/// The f64 is stored as its exact 64-bit pattern (16 hex chars) so a recomputed
/// score round-trips bit-for-bit; a `NoteId` never contains `:`.
fn encode_cursor(score: f64, id: &str) -> String {
    format!("v1:{:016x}:{}", score.to_bits(), id)
}

/// Decodes a cursor token, rejecting anything this index didn't produce.
fn decode_cursor(raw: &str) -> Result<CursorKey> {
    let bad = || IndexError::Cursor {
        value: raw.to_string(),
    };
    let mut parts = raw.splitn(3, ':');
    let version = parts.next().ok_or_else(bad)?;
    let bits_hex = parts.next().ok_or_else(bad)?;
    let id = parts.next().ok_or_else(bad)?;
    if version != "v1" || bits_hex.len() != 16 || id.is_empty() {
        return Err(bad());
    }
    let bits = u64::from_str_radix(bits_hex, 16).map_err(|_| bad())?;
    Ok(CursorKey {
        score: f64::from_bits(bits),
        id: id.to_string(),
    })
}

/// Truncates a snippet to [`SNIPPET_MAX_CHARS`] characters (never a byte
/// boundary), appending an ellipsis only when text was cut.
fn truncate_snippet(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(SNIPPET_MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{EmbedError, FakeEmbedder};
    use crate::index::{EmbeddedChunk, IndexedNote};
    use std::collections::HashSet;

    // --- test fixtures ------------------------------------------------------

    /// Baseline params: a query with every filter at its contract default.
    fn query(text: &str) -> SearchParams {
        SearchParams {
            query: text.to_string(),
            project: None,
            include_descendants: true,
            types: Vec::new(),
            tags: Vec::new(),
            tag_match: TagMatch::Any,
            date_from: None,
            date_to: None,
            limit: 10,
            cursor: None,
        }
    }

    /// A minimal indexed note. Placeholder data only (public-repo rule).
    fn note(
        id: &str,
        project: Option<&str>,
        note_type: NoteType,
        date: &str,
        tags: &[&str],
        title: &str,
        body: &str,
    ) -> IndexedNote {
        IndexedNote {
            id: id.to_string(),
            path: format!("{}/{id}.md", project.unwrap_or("Inbox")),
            title: title.to_string(),
            note_type,
            project: project.map(str::to_string),
            date: date.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            source: "manual".to_string(),
            confidence: None,
            body: body.to_string(),
        }
    }

    fn put(index: &mut NoteIndex, note: &IndexedNote) {
        index.upsert_note(note).unwrap();
    }

    /// A unit vector pointing along one axis — an exact, distinct KNN target.
    fn axis(i: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBEDDING_DIM];
        v[i] = 1.0;
        v
    }

    /// A vector at raw L2 distance `i * 0.1` from `axis(0)` — a deterministic
    /// distance ladder for pagination tests (sqlite-vec orders by raw L2).
    fn near(i: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBEDDING_DIM];
        v[0] = 1.0;
        v[1] = i as f32 * 0.1;
        v
    }

    fn put_vec(index: &mut NoteIndex, note: &IndexedNote, embedding: Vec<f32>, chunk_text: &str) {
        index.upsert_note(note).unwrap();
        index
            .set_note_chunks(
                &note.id,
                &[EmbeddedChunk {
                    text: chunk_text.to_string(),
                    embedding,
                }],
            )
            .unwrap();
    }

    /// A test embedder whose query vector is a fixed embedding, regardless of
    /// text — pairs with `axis`/`near` note vectors for deterministic KNN.
    struct FixedEmbedder(Vec<f32>);

    impl Embedder for FixedEmbedder {
        fn dim(&self) -> usize {
            EMBEDDING_DIM
        }
        fn embed_passages(
            &self,
            texts: &[String],
        ) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
            Ok(texts.iter().map(|_| self.0.clone()).collect())
        }
        fn embed_query(&self, _text: &str) -> std::result::Result<Vec<f32>, EmbedError> {
            Ok(self.0.clone())
        }
    }

    /// An embedder that always fails — models a broken ONNX runtime.
    struct FailingEmbedder;

    impl Embedder for FailingEmbedder {
        fn dim(&self) -> usize {
            EMBEDDING_DIM
        }
        fn embed_passages(
            &self,
            _texts: &[String],
        ) -> std::result::Result<Vec<Vec<f32>>, EmbedError> {
            Err(EmbedError::Backend("boom".to_string()))
        }
        fn embed_query(&self, _text: &str) -> std::result::Result<Vec<f32>, EmbedError> {
            Err(EmbedError::Backend("boom".to_string()))
        }
    }

    fn hit_ids(results: &SearchResults) -> Vec<String> {
        results.hits.iter().map(|h| h.id.clone()).collect()
    }

    fn id_set(results: &SearchResults) -> HashSet<String> {
        results.hits.iter().map(|h| h.id.clone()).collect()
    }

    // --- 1. FTS-only shaping ------------------------------------------------

    #[test]
    fn fts_only_search_shapes_hits_and_ranks() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_aaaaaa",
                Some("Briarwood Golf"),
                NoteType::Meeting,
                "2026-07-09T14:00:00-07:00",
                &["budgeting"],
                "Budget planning",
                "We discussed the quarterly budget in detail",
            ),
        );
        // A note whose title matches but body does not — exercises snippet
        // column auto-selection.
        put(
            &mut index,
            &note(
                "n_titlon",
                Some("Growth"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "Budget overview",
                "unrelated content here",
            ),
        );

        let results = index.search_notes(&query("budget"), None).unwrap();
        assert_eq!(results.hits.len(), 2);
        for hit in &results.hits {
            assert!(hit.score > 0.0);
            assert!(!hit.snippet.is_empty());
            assert!(hit.snippet.contains("**"), "snippet should highlight");
        }

        let top = results
            .hits
            .iter()
            .find(|h| h.id == "n_aaaaaa")
            .expect("body match present");
        assert_eq!(top.note_type, NoteType::Meeting);
        assert_eq!(top.project.as_deref(), Some("Briarwood Golf"));
        // Date is verbatim, offset preserved (not normalized to UTC).
        assert_eq!(top.date, "2026-07-09T14:00:00-07:00");
        assert_eq!(top.tags, vec!["budgeting".to_string()]);
        assert_eq!(top.source, "manual");

        assert_eq!(results.hits[0].rank, 1);
        assert_eq!(results.hits[1].rank, 2);
        assert!(!results.page.has_more);
        assert!(results.page.next_cursor.is_none());
        assert_eq!(results.page.total_estimate, Some(2));
    }

    // --- 2. Fusion ----------------------------------------------------------

    #[test]
    fn fusion_ranks_both_arms_note_above_single_arm_notes() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // A: both arms. Stronger term frequency → FTS rank 1; vector at axis(0)
        // → vector rank 1.
        put_vec(
            &mut index,
            &note(
                "n_aaaa01",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "A",
                "alpha alpha alpha",
            ),
            axis(0),
            "alpha alpha alpha",
        );
        // B: FTS only (has the term, no vector).
        put(
            &mut index,
            &note(
                "n_bbbb01",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "B",
                "alpha",
            ),
        );
        // C: vector only (no term in body, vector present).
        put_vec(
            &mut index,
            &note(
                "n_cccc01",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "C",
                "gamma delta",
            ),
            axis(1),
            "gamma delta",
        );

        let embedder = FixedEmbedder(axis(0));
        let results = index
            .search_notes(&query("alpha"), Some(&embedder))
            .unwrap();
        assert_eq!(hit_ids(&results), vec!["n_aaaa01", "n_bbbb01", "n_cccc01"]);

        let a = &results.hits[0];
        assert_eq!(a.rank, 1);
        assert!(
            (a.score - 2.0 / 61.0).abs() < 1e-12,
            "both arms: 1/61 + 1/61"
        );
        // B and C tie at 1/62; the id tiebreak orders B before C.
        assert!((results.hits[1].score - 1.0 / 62.0).abs() < 1e-12);
        assert!((results.hits[2].score - 1.0 / 62.0).abs() < 1e-12);
    }

    // --- 3. Vector-only snippet --------------------------------------------

    #[test]
    fn vector_only_hit_carries_chunk_text_snippet() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put_vec(
            &mut index,
            &note(
                "n_vec001",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "V",
                "some body",
            ),
            axis(0),
            "the exact chunk words",
        );

        let embedder = FixedEmbedder(axis(0));
        // A query term that isn't in any body → FTS arm finds nothing.
        let results = index
            .search_notes(&query("nonexistentterm"), Some(&embedder))
            .unwrap();
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.hits[0].snippet, "the exact chunk words");
        assert!(!results.hits[0].snippet.contains("**"));
    }

    // --- 4. Multi-chunk dedupe ---------------------------------------------

    #[test]
    fn multi_chunk_note_dedupes_to_nearest_chunk_snippet() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_multi1",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "M",
                "body",
            ),
        );
        index
            .set_note_chunks(
                "n_multi1",
                &[
                    EmbeddedChunk {
                        text: "chunk zero".to_string(),
                        embedding: axis(0),
                    },
                    EmbeddedChunk {
                        text: "chunk five".to_string(),
                        embedding: axis(5),
                    },
                ],
            )
            .unwrap();

        let embedder = FixedEmbedder(axis(0));
        let results = index
            .search_notes(&query("nonexistentterm"), Some(&embedder))
            .unwrap();
        assert_eq!(results.hits.len(), 1, "one hit per note");
        assert_eq!(results.hits[0].snippet, "chunk zero", "nearest chunk wins");
    }

    // --- 5. Tie-break -------------------------------------------------------

    #[test]
    fn equal_scores_tie_break_by_id_ascending() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // X: FTS rank 1 only. Y: vector rank 1 only. Both score 1/61.
        put(
            &mut index,
            &note(
                "n_xxxx99",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "X",
                "alpha",
            ),
        );
        put_vec(
            &mut index,
            &note(
                "n_yyyy01",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "Y",
                "beta",
            ),
            axis(0),
            "beta",
        );

        let embedder = FixedEmbedder(axis(0));
        let results = index
            .search_notes(&query("alpha"), Some(&embedder))
            .unwrap();
        assert_eq!(hit_ids(&results), vec!["n_xxxx99", "n_yyyy01"]);
        assert!((results.hits[0].score - results.hits[1].score).abs() < 1e-12);
        assert!((results.hits[0].score - 1.0 / 61.0).abs() < 1e-12);
    }

    // --- 6. Project filter --------------------------------------------------

    #[test]
    fn project_filter_matches_exact_and_descendants() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_g1",
                Some("Growth"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "G1",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_g2",
                Some("Growth/Q3"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "G2",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_g3",
                Some("Growthx"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "G3",
                "report",
            ),
        );

        let mut params = query("report");
        params.project = Some("Growth".to_string());
        let results = index.search_notes(&params, None).unwrap();
        assert_eq!(
            id_set(&results),
            HashSet::from(["n_g1".to_string(), "n_g2".to_string()]),
            "sibling 'Growthx' must not match"
        );

        params.include_descendants = false;
        let results = index.search_notes(&params, None).unwrap();
        assert_eq!(hit_ids(&results), vec!["n_g1"]);
    }

    #[test]
    fn project_filter_escapes_like_wildcards() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_u1",
                Some("a_b"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "U1",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_u2",
                Some("a_b/child"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "U2",
                "report",
            ),
        );
        // 'axb' would match an unescaped `a_b/%` pattern (`_` as wildcard).
        put(
            &mut index,
            &note(
                "n_u3",
                Some("axb"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "U3",
                "report",
            ),
        );

        let mut params = query("report");
        params.project = Some("a_b".to_string());
        let results = index.search_notes(&params, None).unwrap();
        assert_eq!(
            id_set(&results),
            HashSet::from(["n_u1".to_string(), "n_u2".to_string()])
        );
    }

    // --- 7. Inbox filter ----------------------------------------------------

    #[test]
    fn inbox_project_filter_matches_unfiled_notes() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_in1",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "In",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_pr1",
                Some("Growth"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "Pr",
                "report",
            ),
        );

        for sentinel in ["Inbox", "inbox"] {
            let mut params = query("report");
            params.project = Some(sentinel.to_string());
            let results = index.search_notes(&params, None).unwrap();
            assert_eq!(hit_ids(&results), vec!["n_in1"], "sentinel {sentinel:?}");
        }
    }

    // --- 8. Type filter -----------------------------------------------------

    #[test]
    fn type_filter_restricts_results() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_mt",
                None,
                NoteType::Meeting,
                "2026-07-01",
                &[],
                "Mt",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_nt",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "Nt",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_ct",
                None,
                NoteType::Chat,
                "2026-07-01",
                &[],
                "Ct",
                "report",
            ),
        );

        let mut params = query("report");
        params.types = vec![NoteType::Meeting, NoteType::Chat];
        let results = index.search_notes(&params, None).unwrap();
        assert_eq!(
            id_set(&results),
            HashSet::from(["n_mt".to_string(), "n_ct".to_string()])
        );
    }

    // --- 9. Tag filter ------------------------------------------------------

    #[test]
    fn tag_match_any_vs_all() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_t1",
                None,
                NoteType::Note,
                "2026-07-01",
                &["a", "b"],
                "T1",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_t2",
                None,
                NoteType::Note,
                "2026-07-01",
                &["a"],
                "T2",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_t3",
                None,
                NoteType::Note,
                "2026-07-01",
                &["b", "c"],
                "T3",
                "report",
            ),
        );

        let mut params = query("report");
        params.tags = vec!["a".to_string(), "b".to_string()];
        params.tag_match = TagMatch::Any;
        let any = index.search_notes(&params, None).unwrap();
        assert_eq!(
            id_set(&any),
            HashSet::from(["n_t1".to_string(), "n_t2".to_string(), "n_t3".to_string()])
        );

        params.tag_match = TagMatch::All;
        let all = index.search_notes(&params, None).unwrap();
        assert_eq!(hit_ids(&all), vec!["n_t1"]);
    }

    // --- 10. Date bounds ----------------------------------------------------

    #[test]
    fn date_bounds_are_inclusive_and_utc_normalized() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Local date-only, normalizes to 2026-07-10T00:00:00Z.
        put(
            &mut index,
            &note(
                "n_d1",
                None,
                NoteType::Note,
                "2026-07-10",
                &[],
                "D1",
                "report",
            ),
        );
        // Wall clock 2026-07-09 20:00 -07:00 is 2026-07-10T03:00:00Z — inside
        // the 7/10 bound despite its local calendar day being 7/9.
        put(
            &mut index,
            &note(
                "n_d2",
                None,
                NoteType::Note,
                "2026-07-09T20:00:00-07:00",
                &[],
                "D2",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_d3",
                None,
                NoteType::Note,
                "2026-07-11",
                &[],
                "D3",
                "report",
            ),
        );

        let mut params = query("report");
        params.date_from = Some("2026-07-10".to_string());
        params.date_to = Some("2026-07-10".to_string());
        let results = index.search_notes(&params, None).unwrap();
        assert_eq!(
            id_set(&results),
            HashSet::from(["n_d1".to_string(), "n_d2".to_string()]),
            "UTC day, inclusive both ends"
        );
    }

    #[test]
    fn malformed_date_bound_is_an_error() {
        let index = NoteIndex::open_in_memory().unwrap();
        let mut params = query("report");
        params.date_from = Some("not-a-date".to_string());
        assert!(matches!(
            index.search_notes(&params, None),
            Err(IndexError::Date { .. })
        ));
    }

    // --- 11. Filters reach the vector arm -----------------------------------

    #[test]
    fn filters_apply_to_the_vector_arm() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put_vec(
            &mut index,
            &note(
                "n_v1",
                Some("Growth"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "V1",
                "xyz",
            ),
            axis(0),
            "xyz",
        );
        // Semantically identical (same vector) but in an excluded project.
        put_vec(
            &mut index,
            &note(
                "n_v2",
                Some("Other"),
                NoteType::Note,
                "2026-07-01",
                &[],
                "V2",
                "xyz",
            ),
            axis(0),
            "xyz",
        );

        let embedder = FixedEmbedder(axis(0));
        let mut params = query("unrelatedword");
        params.project = Some("Growth".to_string());
        let results = index.search_notes(&params, Some(&embedder)).unwrap();
        assert_eq!(
            hit_ids(&results),
            vec!["n_v1"],
            "vector arm honors the filter"
        );
    }

    // --- 12. Limit clamping -------------------------------------------------

    #[test]
    fn limit_clamps_out_of_range_values() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        for i in 0..3 {
            put(
                &mut index,
                &note(
                    &format!("n_l{i}"),
                    None,
                    NoteType::Note,
                    "2026-07-01",
                    &[],
                    "L",
                    "report",
                ),
            );
        }

        let mut params = query("report");
        params.limit = 0;
        let low = index.search_notes(&params, None).unwrap();
        assert_eq!(low.hits.len(), 1, "0 clamps up to 1");
        assert!(low.page.has_more);

        params.limit = 500;
        let high = index.search_notes(&params, None).unwrap();
        assert_eq!(
            high.hits.len(),
            3,
            "500 clamps down to 50, capped by matches"
        );
        assert!(!high.page.has_more);
    }

    // --- 13 & 14. Pagination ------------------------------------------------

    #[test]
    fn pagination_walks_the_fused_ranking_without_gaps() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Five vector-only notes on a strict distance ladder → ranks 1..=5.
        for i in 1..=5 {
            put_vec(
                &mut index,
                &note(
                    &format!("n_p{i}"),
                    None,
                    NoteType::Note,
                    "2026-07-01",
                    &[],
                    "P",
                    "body",
                ),
                near(i),
                &format!("chunk {i}"),
            );
        }

        let embedder = FixedEmbedder(axis(0));
        let mut params = query("nomatchterm");
        params.limit = 2;

        let mut seen = Vec::new();
        let mut ranks = Vec::new();
        loop {
            let page = index.search_notes(&params, Some(&embedder)).unwrap();
            for hit in &page.hits {
                seen.push(hit.id.clone());
                ranks.push(hit.rank);
            }
            match page.page.next_cursor {
                Some(cursor) => params.cursor = Some(cursor),
                None => {
                    assert!(!page.page.has_more);
                    break;
                }
            }
        }

        assert_eq!(
            seen,
            vec!["n_p1", "n_p2", "n_p3", "n_p4", "n_p5"],
            "continuous, no overlap or gaps"
        );
        assert_eq!(ranks, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn pagination_survives_mutation_between_pages() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        for i in 1..=5 {
            put_vec(
                &mut index,
                &note(
                    &format!("n_m{i}"),
                    None,
                    NoteType::Note,
                    "2026-07-01",
                    &[],
                    "M",
                    "body",
                ),
                near(i),
                &format!("chunk {i}"),
            );
        }

        let embedder = FixedEmbedder(axis(0));
        let mut params = query("nomatchterm");
        params.limit = 2;
        let page1 = index.search_notes(&params, Some(&embedder)).unwrap();
        let page1_ids: HashSet<String> = id_set(&page1);

        // Mutate under the cursor: delete a page-1 note, insert a fresh one.
        index.delete_note("n_m1").unwrap();
        put_vec(
            &mut index,
            &note(
                "n_m9",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "M9",
                "body",
            ),
            near(9),
            "chunk nine",
        );

        params.cursor = page1.page.next_cursor;
        let page2 = index.search_notes(&params, Some(&embedder)).unwrap();
        // No panic, and nothing at-or-before the cursor is re-served.
        assert!(page2.hits.iter().all(|h| !page1_ids.contains(&h.id)));
    }

    // --- 15. Cursor tampering -----------------------------------------------

    #[test]
    fn tampered_cursor_is_an_error() {
        let index = NoteIndex::open_in_memory().unwrap();
        for bad in [
            "garbage",
            "v2:0000000000000000:n_x",
            "v1:xyz:n_x",
            "v1:00:n_x",
            "v1:0000000000000000:",
        ] {
            let mut params = query("report");
            params.cursor = Some(bad.to_string());
            assert!(
                matches!(
                    index.search_notes(&params, None),
                    Err(IndexError::Cursor { .. })
                ),
                "cursor {bad:?} should be rejected"
            );
        }
    }

    // --- 16. Query safety ---------------------------------------------------

    #[test]
    fn special_character_queries_never_error() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put_vec(
            &mut index,
            &note(
                "n_s1",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "S",
                "some body text",
            ),
            axis(0),
            "some body text",
        );

        let embedder = FixedEmbedder(axis(0));
        for text in [
            "don't",
            "title:foo",
            "NEAR(",
            "a OR b",
            "\"quoted\"",
            "(((",
            "*",
            "?!?",
        ] {
            let result = index.search_notes(&query(text), Some(&embedder));
            assert!(result.is_ok(), "query {text:?} must not error");
        }
    }

    // --- 17. Empty query ----------------------------------------------------

    #[test]
    fn empty_or_whitespace_query_is_an_empty_success() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_e1",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "E",
                "report",
            ),
        );

        for text in ["", "   ", "\t\n"] {
            let results = index
                .search_notes(&query(text), Some(&FakeEmbedder))
                .unwrap();
            assert!(results.hits.is_empty(), "query {text:?}");
            assert!(!results.page.has_more);
            assert!(results.page.next_cursor.is_none());
            assert_eq!(results.page.total_estimate, Some(0));
        }
    }

    // --- 18 & 19. Embedder degradation --------------------------------------

    #[test]
    fn degrades_to_fts_only_without_a_working_vector_arm() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // FTS content, no vectors.
        put(
            &mut index,
            &note(
                "n_f1",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "F",
                "report",
            ),
        );

        // No embedder.
        assert_eq!(
            index
                .search_notes(&query("report"), None)
                .unwrap()
                .hits
                .len(),
            1
        );
        // Embedder present, but the vector table is empty.
        assert_eq!(
            index
                .search_notes(&query("report"), Some(&FakeEmbedder))
                .unwrap()
                .hits
                .len(),
            1
        );
        // Broken embedder → still FTS-only, still Ok.
        assert_eq!(
            index
                .search_notes(&query("report"), Some(&FailingEmbedder))
                .unwrap()
                .hits
                .len(),
            1
        );
    }

    // --- 20. Serialized shape -----------------------------------------------

    #[test]
    fn serialized_output_matches_the_contract() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        put(
            &mut index,
            &note(
                "n_json1",
                Some("Growth"),
                NoteType::Meeting,
                "2026-07-10",
                &["x"],
                "J1",
                "report",
            ),
        );
        // An unfiled note with no confidence → null project + confidence.
        put(
            &mut index,
            &note(
                "n_json2",
                None,
                NoteType::Note,
                "2026-07-10",
                &[],
                "J2",
                "report",
            ),
        );

        let results = index.search_notes(&query("report"), None).unwrap();
        let value = serde_json::to_value(&results).unwrap();

        let expected_hit_keys: HashSet<&str> = [
            "id",
            "path",
            "title",
            "type",
            "project",
            "date",
            "tags",
            "source",
            "confidence",
            "score",
            "rank",
            "snippet",
        ]
        .into_iter()
        .collect();
        for hit in value["hits"].as_array().unwrap() {
            let keys: HashSet<&str> = hit
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(keys, expected_hit_keys);
        }

        let filed = value["hits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["id"] == "n_json1")
            .unwrap();
        assert_eq!(filed["type"], "meeting");
        assert_eq!(filed["project"], "Growth");

        let unfiled = value["hits"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["id"] == "n_json2")
            .unwrap();
        assert!(unfiled["project"].is_null());
        assert!(unfiled["confidence"].is_null());

        let page = value["page"].as_object().unwrap();
        let page_keys: HashSet<&str> = page.keys().map(String::as_str).collect();
        assert_eq!(
            page_keys,
            HashSet::from(["has_more", "next_cursor", "total_estimate"])
        );
        assert!(page["next_cursor"].is_null());
    }

    // --- 21. Params defaults ------------------------------------------------

    #[test]
    fn search_params_deserialize_with_contract_defaults() {
        let params: SearchParams =
            serde_json::from_value(serde_json::json!({"query": "x"})).unwrap();
        assert!(params.include_descendants);
        assert_eq!(params.tag_match, TagMatch::Any);
        assert_eq!(params.limit, 10);
        assert!(params.types.is_empty());
        assert!(params.tags.is_empty());

        // additionalProperties: false — an unknown field is rejected.
        assert!(serde_json::from_value::<SearchParams>(
            serde_json::json!({"query": "x", "bogus": 1})
        )
        .is_err());
    }

    // --- 22. Snippet truncation ---------------------------------------------

    #[test]
    fn long_chunk_snippet_truncates_at_a_char_boundary() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let long = "é".repeat(400); // multibyte, longer than the budget
        put_vec(
            &mut index,
            &note(
                "n_long1",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "L",
                "plain",
            ),
            axis(0),
            &long,
        );

        let embedder = FixedEmbedder(axis(0));
        let results = index
            .search_notes(&query("zzznomatch"), Some(&embedder))
            .unwrap();
        let snippet = &results.hits[0].snippet;
        assert_eq!(
            snippet.chars().count(),
            SNIPPET_MAX_CHARS + 1,
            "budget + ellipsis"
        );
        assert!(snippet.ends_with('…'));
    }
}
