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
//! ## Pagination
//!
//! An RRF score is derived from a note's *position* in each arm, not from
//! anything intrinsic to the note, so every score in the list shifts when the
//! corpus changes or when an arm drops out. That makes a stored score a poor
//! resume key on its own. Two things keep the walk exact anyway:
//!
//! - A cursor names the boundary **note**, and the resume point is re-read from
//!   *this* run's fused list by that id. The encoded score is only a fallback
//!   for a boundary note that has since been deleted. So a note inserted above
//!   the cursor — which shifts every score below it — no longer re-serves or
//!   skips the rows around the boundary, and neither does the vector arm
//!   dropping out mid-walk.
//! - A cursor also carries how many positions have been served, and each arm
//!   fetches that many candidates plus [`CANDIDATE_HEADROOM`]. The pool
//!   therefore always extends past the page being built, so `has_more` reports
//!   the corpus running out rather than the pool running out — up to the
//!   [`MAX_CANDIDATES`] ceiling, past which `total_estimate` goes `null` to say
//!   the count is no longer known.
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
use rusqlite::{params_from_iter, Row};

use super::embed::embedding_to_blob;
use super::note::NoteType;
use super::{IndexError, NoteIndex, Result, EMBEDDING_DIM};
use crate::embed::Embedder;

/// The RRF constant. 60 is the value from the original Cormack et al. RRF paper
/// and the de-facto default; the contract leaves it to the implementer. Larger
/// values flatten the contribution of top ranks (less weight on being #1).
const RRF_K: f64 = 60.0;

/// How far past the page being built each arm fetches candidates. The surplus
/// is what lets `has_more` mean "the corpus has more" rather than "the pool
/// ended here": if an arm comes back with fewer than the requested depth, its
/// ranking really is exhausted.
const CANDIDATE_HEADROOM: usize = 200;

/// Multiplier applied to the vector arm's *chunk* depth before per-note dedup
/// and filtering. Over-fetched because a note can own several chunks and
/// because post-filtering shrinks the pool: v1 uses a fixed over-fetch rather
/// than iterative widening, so under a highly selective filter only
/// *semantic-only* recall degrades — the FTS arm still filters over the whole
/// corpus in SQL.
const VEC_CHUNK_OVERFETCH: usize = 2;

/// Ceiling on candidate depth however deep the caller pages. Each page re-runs
/// both arms at `served + limit + headroom`, so an unbounded walk would grow
/// without limit; past this depth the search stops widening and says so by
/// reporting a `null` `total_estimate`.
const MAX_CANDIDATES: usize = 5_000;

/// Cap on query tokens fed to the FTS arm — a guard against a pathological
/// multi-kilobyte query, not a limit real queries hit.
const MAX_QUERY_TOKENS: usize = 64;

/// Cap on distinct values in a `tags` or `type` filter. Each value binds one
/// SQL parameter in both arms, and SQLite's `SQLITE_MAX_VARIABLE_NUMBER` is 999
/// on common builds — without this, a large array fails deep in the driver with
/// an opaque "too many SQL variables". Overflow is an error rather than a
/// truncation because dropping a value from a `tag_match: all` filter would
/// quietly widen it into a different question.
const MAX_FILTER_VALUES: usize = 64;

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

/// Cursor-based pagination envelope. Cursors are opaque and name the boundary
/// *note*, not an offset, so a note added or removed elsewhere in the ranking
/// between pages never re-serves or skips the rows around that boundary (see
/// the module's Pagination section for why the encoded score alone would not be
/// enough). A note that is inserted *above* the boundary after the caller has
/// already passed that position is simply not seen on this walk — the standard
/// keyset trade, and the one the contract asks for.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PageInfo {
    /// True if more results exist beyond this page.
    pub has_more: bool,
    /// Token to pass as the next request's `cursor`; `null` when `has_more` is
    /// false.
    pub next_cursor: Option<String>,
    /// Exact match count when both arms' rankings were exhausted, or `null`
    /// once a candidate pool came back full — past that point the true total is
    /// not known, and reporting the pool size would understate it.
    pub total_estimate: Option<u64>,
}

/// The `search_notes` output — ranked `hits` plus the pagination `page`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    pub page: PageInfo,
}

/// A decoded pagination cursor: which note the prior page ended on, and how
/// many fused positions have been served so far.
struct CursorKey {
    /// The boundary note's fused score *as of the prior page* — a fallback used
    /// only when that note is no longer in the fused list.
    score: f64,
    /// Fused positions consumed by prior pages. Sizes this run's candidate
    /// pools; never used to locate the boundary, so a stale value costs a
    /// little work and cannot misplace a page.
    served: usize,
    id: String,
}

/// The `notes` columns a [`SearchHit`] needs, in [`map_hit_row`]'s order.
///
/// Deliberately narrower than `query::NOTE_COLUMNS`: a hit carries a snippet
/// rather than a body, so hydrating through the full list would read every
/// hit's complete text — megabytes for a page of meeting notes — only to drop
/// it. `date_utc` is likewise omitted; hits report the verbatim `date_raw`.
const HIT_COLUMNS: &str = "id, path, title, type, project, date_raw, source, confidence";

/// A hydrated `notes` row for one hit. `tags` are filled separately, batched
/// across the page.
struct HitRow {
    id: String,
    path: String,
    title: String,
    note_type: NoteType,
    project: Option<String>,
    date: String,
    source: String,
    confidence: Option<f64>,
    tags: Vec<String>,
}

/// Builds a [`HitRow`] from a `SELECT` of [`HIT_COLUMNS`].
fn map_hit_row(row: &Row<'_>) -> rusqlite::Result<HitRow> {
    Ok(HitRow {
        id: row.get("id")?,
        path: row.get("path")?,
        title: row.get("title")?,
        note_type: row.get("type")?,
        project: row.get("project")?,
        date: row.get("date_raw")?,
        source: row.get("source")?,
        confidence: row.get("confidence")?,
        tags: Vec::new(),
    })
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

        // Validate every input before the empty-query shortcut, so a malformed
        // cursor or date bound is rejected the same way whatever the query says.
        let filters = build_filters(params)?;
        let cursor = params.cursor.as_deref().map(decode_cursor).transpose()?;

        // A query with nothing to match on: succeed with an empty page rather
        // than error — absence is a valid answer (MCP_TOOL_SURFACE.md §2).
        if params.query.trim().is_empty() {
            return Ok(empty_results());
        }

        // Candidate depth covers everything already served plus this page plus
        // headroom, so the pool outruns the walk instead of ending under it.
        let served = cursor.as_ref().map_or(0, |key| key.served);
        let depth = served
            .saturating_add(limit)
            .saturating_add(CANDIDATE_HEADROOM)
            .min(MAX_CANDIDATES);

        // Arm 1: full-text. Skipped when sanitizing leaves no usable term, so no
        // invalid FTS5 MATCH expression ever reaches SQLite.
        let fts_expr = sanitize_fts_query(&params.query);
        let fts = match &fts_expr {
            Some(expr) => self.fts_arm(expr, &filters, depth)?,
            None => Vec::new(),
        };

        // Arm 2: semantic. Degrades to empty on any embedding problem.
        let vector = self.vector_candidates(&params.query, embedder, &filters, depth)?;

        // An arm that came back full was cut off, so the fused list is a prefix
        // of the true ranking. `exact` gates `total_estimate`; `widenable` gates
        // `has_more`, and excludes the ceiling case so the walk still ends.
        let pools_full = fts.len() >= depth || vector.len() >= depth;
        let exact = !pools_full;
        let widenable = pools_full && depth < MAX_CANDIDATES;

        // Fuse via RRF: a note's score sums 1/(k+rank) over the arms it appears
        // in, so both-arms notes rise above single-arm ones.
        let mut scores: HashMap<&str, f64> = HashMap::new();
        for (rank0, id) in fts.iter().enumerate() {
            *scores.entry(id.as_str()).or_insert(0.0) += rrf_term(rank0);
        }
        for (rank0, (id, _)) in vector.iter().enumerate() {
            *scores.entry(id.as_str()).or_insert(0.0) += rrf_term(rank0);
        }

        // Total order: score descending, ties by id ascending — the id tiebreak
        // is load-bearing because equal RRF scores are common and pagination
        // needs a deterministic order.
        let mut fused: Vec<(&str, f64)> = scores.into_iter().collect();
        fused.sort_by(fused_order);
        let total = fused.len();

        // Keyset pagination: resume strictly after the boundary note. The
        // boundary's key is re-read from *this* run's list, because an RRF score
        // is a function of rank — one note inserted above the cursor shifts
        // every score below it, and trusting the encoded score would then land
        // the boundary in the wrong place and re-serve a row. The encoded score
        // is the fallback for a boundary note that has since been deleted.
        let start = match &cursor {
            Some(key) => {
                let boundary = fused
                    .iter()
                    .find(|(id, _)| *id == key.id.as_str())
                    .copied()
                    .unwrap_or((key.id.as_str(), key.score));
                fused.partition_point(|item| fused_order(item, &boundary) != Ordering::Greater)
            }
            None => 0,
        };
        let end = (start + limit).min(total);
        let page = &fused[start..end];
        let page_ids: Vec<&str> = page.iter().map(|(id, _)| *id).collect();

        // Hydrate only the page: one row fetch, one batched tag load, and
        // snippets sourced per hit below.
        let mut rows = self.load_hit_rows(&page_ids)?;
        let mut snippets = self.page_snippets(fts_expr.as_deref(), &fts, &vector, &page_ids)?;

        let mut hits = Vec::with_capacity(page.len());
        for (offset, (id, score)) in page.iter().enumerate() {
            // A row can vanish if a concurrent writer deletes it between the arm
            // query and hydration — drop it from the page rather than fail.
            let Some(row) = rows.remove(*id) else {
                continue;
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
                snippet: snippets.remove(*id).unwrap_or_default(),
            });
        }

        // More results exist if the fused list has some, or if it was cut off
        // and a deeper re-fetch could reveal some.
        let has_more = end < total || (widenable && end > 0);
        // Encode from the last *fused* position consumed (not the last hydrated
        // hit) so a concurrently-deleted boundary row still advances the cursor.
        let next_cursor = has_more.then(|| {
            let (id, score) = fused[end - 1];
            encode_cursor(score, end, id)
        });

        Ok(SearchResults {
            hits,
            page: PageInfo {
                has_more,
                next_cursor,
                total_estimate: exact.then_some(total as u64),
            },
        })
    }

    /// Runs the FTS arm: the ids of notes matching `match_expr` (under
    /// `filters`), best `bm25` first, capped at `depth`.
    ///
    /// Ids only — `snippet()` re-reads and re-tokenizes the row's text out of
    /// the external content table, so computing it for every candidate would
    /// pay that cost hundreds of times per query to keep at most `limit` of the
    /// results. [`page_snippets`](Self::page_snippets) does it once the page is
    /// known.
    fn fts_arm(&self, match_expr: &str, filters: &Filters, depth: usize) -> Result<Vec<String>> {
        let sql = format!(
            "SELECT notes.id
             FROM notes_fts JOIN notes ON notes.pk = notes_fts.rowid
             WHERE notes_fts MATCH ?{filters}
             ORDER BY bm25(notes_fts) LIMIT ?",
            filters = filters.where_and()
        );

        // Bind order follows the placeholders: MATCH expr, filter values, LIMIT.
        let mut values = Vec::with_capacity(filters.values.len() + 2);
        values.push(Value::Text(match_expr.to_string()));
        values.extend(filters.values.iter().cloned());
        values.push(Value::Integer(depth as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    }

    /// One snippet per hit on the page, keyed by note id.
    ///
    /// An FTS hit gets the query-aware highlighted `snippet()`; a vector-only
    /// hit gets its nearest chunk's own words, truncated. Both are looked up in
    /// a single batched query over just the page's ids.
    fn page_snippets(
        &self,
        fts_expr: Option<&str>,
        fts: &[String],
        vector: &[(String, i64)],
        page_ids: &[&str],
    ) -> Result<HashMap<String, String>> {
        let fts_ids: HashSet<&str> = fts.iter().map(String::as_str).collect();
        let from_fts: Vec<&str> = page_ids
            .iter()
            .copied()
            .filter(|id| fts_ids.contains(id))
            .collect();

        let mut snippets = match fts_expr {
            Some(expr) if !from_fts.is_empty() => self.fts_snippets(expr, &from_fts)?,
            _ => HashMap::new(),
        };

        // Whatever the FTS arm didn't cover falls back to the note's nearest
        // chunk: vector-only hits, plus the rare FTS hit whose row was deleted
        // between the arm query and the snippet lookup.
        let vec_seq: HashMap<&str, i64> =
            vector.iter().map(|(id, seq)| (id.as_str(), *seq)).collect();
        let chunk_keys: Vec<(&str, i64)> = page_ids
            .iter()
            .filter(|id| !snippets.contains_key(**id))
            .filter_map(|id| vec_seq.get(id).map(|seq| (*id, *seq)))
            .collect();

        for (note_id, text) in self.chunk_texts(&chunk_keys)? {
            snippets.insert(note_id, truncate_snippet(&text));
        }
        Ok(snippets)
    }

    /// Highlighted `snippet()` text for the given note ids, in one query. The
    /// ids are already filtered by the arm that produced them, so this repeats
    /// only the `MATCH` — the filter clauses would be redundant.
    fn fts_snippets(&self, match_expr: &str, ids: &[&str]) -> Result<HashMap<String, String>> {
        let sql = format!(
            "SELECT notes.id, snippet(notes_fts, -1, '**', '**', '…', 16)
             FROM notes_fts JOIN notes ON notes.pk = notes_fts.rowid
             WHERE notes_fts MATCH ? AND notes.id IN ({placeholders})",
            placeholders = sql_placeholders(ids.len())
        );

        let mut values = Vec::with_capacity(ids.len() + 1);
        values.push(Value::Text(match_expr.to_string()));
        values.extend(ids.iter().map(|id| Value::Text((*id).to_string())));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<String, String>>>()?;
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
        depth: usize,
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
        self.vector_arm(&query_vec, filters, depth)
    }

    /// Runs the KNN vector arm: the chunks nearest `query_vec` (under
    /// `filters`), deduped to the nearest chunk per note. Returns `(note_id,
    /// seq)` in distance order, capped at `depth` notes. The `seq` is the
    /// nearest chunk's index, used to source the note's snippet.
    fn vector_arm(
        &self,
        query_vec: &[f32],
        filters: &Filters,
        depth: usize,
    ) -> Result<Vec<(String, i64)>> {
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
        let chunk_depth = depth.saturating_mul(VEC_CHUNK_OVERFETCH);
        let mut values = Vec::with_capacity(filters.values.len() + 2);
        values.push(Value::Blob(embedding_to_blob(query_vec)));
        values.push(Value::Integer(chunk_depth as i64));
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
                if out.len() == depth {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Loads the [`HitRow`]s for a page of ids (with tags), keyed by id. Ids
    /// absent from the index — deleted since the arm queries ran — simply don't
    /// appear in the map.
    fn load_hit_rows(&self, ids: &[&str]) -> Result<HashMap<String, HitRow>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let sql = format!(
            "SELECT {HIT_COLUMNS} FROM notes WHERE id IN ({placeholders})",
            placeholders = sql_placeholders(ids.len())
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(ids.iter()), map_hit_row)?
            .collect::<rusqlite::Result<Vec<HitRow>>>()?;

        let mut by_id: HashMap<String, HitRow> =
            rows.into_iter().map(|row| (row.id.clone(), row)).collect();
        let present: Vec<&str> = by_id.keys().map(String::as_str).collect();
        let mut tags = self.load_tags_by_ids(&present)?;
        for (id, row) in &mut by_id {
            row.tags = tags.remove(id).unwrap_or_default();
        }
        Ok(by_id)
    }

    /// The stored text of each `(note_id, seq)` chunk, keyed by note id, in one
    /// query. A key with no row — a note re-chunked between the vector query
    /// and this lookup — is simply absent from the map.
    fn chunk_texts(&self, keys: &[(&str, i64)]) -> Result<HashMap<String, String>> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        // One `OR`-ed pair per key rather than a row-value `IN`, which needs a
        // newer SQLite than the bundled floor guarantees.
        let predicate = std::iter::repeat_n("(note_id = ? AND seq = ?)", keys.len())
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!("SELECT note_id, text FROM note_chunks WHERE {predicate}");

        let mut values = Vec::with_capacity(keys.len() * 2);
        for (note_id, seq) in keys {
            values.push(Value::Text((*note_id).to_string()));
            values.push(Value::Integer(*seq));
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<HashMap<String, String>>>()?;
        Ok(rows)
    }
}

/// One arm's RRF contribution for a 0-based rank: `1 / (k + rank)`.
fn rrf_term(rank0: usize) -> f64 {
    1.0 / (RRF_K + (rank0 + 1) as f64)
}

/// The fused total order: score descending, ties broken by id ascending.
/// `total_cmp` gives a deterministic total order over the f64 scores (no NaN is
/// produced, but this keeps the sort total and clippy-clean).
fn fused_order(a: &(&str, f64), b: &(&str, f64)) -> Ordering {
    b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0))
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

    // Both arrays are deduplicated first. The schema marks them `uniqueItems`,
    // but nothing enforces that on the wire, and a repeat is not harmless: the
    // `all` branch below compares a DISTINCT count against the list length, so
    // `["a", "a"]` would demand two distinct tags from a note that can only
    // ever supply one, and silently match nothing.
    let types = dedup_types(&params.types);
    check_filter_size("type", types.len())?;
    if !types.is_empty() {
        let placeholders = sql_placeholders(types.len());
        clauses.push(format!("notes.type IN ({placeholders})"));
        for note_type in &types {
            values.push(Value::Text(note_type.as_str().to_string()));
        }
    }

    let tags = dedup_strings(&params.tags);
    check_filter_size("tags", tags.len())?;
    if !tags.is_empty() {
        let placeholders = sql_placeholders(tags.len());
        match params.tag_match {
            TagMatch::Any => {
                clauses.push(format!(
                    "EXISTS (SELECT 1 FROM note_tags
                     WHERE note_tags.note_pk = notes.pk AND note_tags.tag IN ({placeholders}))"
                ));
                for tag in &tags {
                    values.push(Value::Text((*tag).to_string()));
                }
            }
            TagMatch::All => {
                clauses.push(format!(
                    "(SELECT count(DISTINCT note_tags.tag) FROM note_tags
                      WHERE note_tags.note_pk = notes.pk AND note_tags.tag IN ({placeholders}))
                     = ?"
                ));
                for tag in &tags {
                    values.push(Value::Text((*tag).to_string()));
                }
                values.push(Value::Integer(tags.len() as i64));
            }
        }
    }

    // Date bounds are the note's *local* calendar day — the one the frontmatter
    // `date` shows the user — not its UTC instant. `date_raw` starts with
    // `YYYY-MM-DD` in both shapes the schema accepts (date-only, or RFC 3339
    // with an offset), so its first 10 characters are exactly that day.
    // Comparing against `date_utc` instead would push an evening note into the
    // following day: `2026-07-09T20:00:00-07:00` is local July 9 but
    // `2026-07-10T03:00:00Z` in UTC, and a `date_from: 2026-07-10` filter would
    // wrongly return it. Inclusive both ends.
    if let Some(from) = &params.date_from {
        clauses.push("substr(notes.date_raw, 1, 10) >= ?".to_string());
        values.push(Value::Text(parse_iso_date(from)?));
    }
    if let Some(to) = &params.date_to {
        clauses.push("substr(notes.date_raw, 1, 10) <= ?".to_string());
        values.push(Value::Text(parse_iso_date(to)?));
    }

    Ok(Filters { clauses, values })
}

/// Rejects a filter array too large to bind (see [`MAX_FILTER_VALUES`]).
fn check_filter_size(field: &'static str, got: usize) -> Result<()> {
    if got > MAX_FILTER_VALUES {
        return Err(IndexError::FilterTooLarge {
            field,
            max: MAX_FILTER_VALUES,
            got,
        });
    }
    Ok(())
}

/// The distinct values of `tags`, in first-seen order.
fn dedup_strings(tags: &[String]) -> Vec<&str> {
    let mut seen = HashSet::new();
    tags.iter()
        .map(String::as_str)
        .filter(|tag| seen.insert(*tag))
        .collect()
}

/// The distinct values of `types`, in first-seen order. A linear scan rather
/// than a set: [`NoteType`] has three variants, so the list is tiny.
fn dedup_types(types: &[NoteType]) -> Vec<NoteType> {
    let mut unique: Vec<NoteType> = Vec::new();
    for note_type in types {
        if !unique.contains(note_type) {
            unique.push(*note_type);
        }
    }
    unique
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

/// Encodes a cursor: the boundary note's fallback `score` bits, how many fused
/// positions have been served, and the note's id. The f64 is stored as its
/// exact 64-bit pattern (16 hex chars) so it round-trips bit-for-bit; a
/// `NoteId` never contains `:`, so it can occupy the unbounded last field.
fn encode_cursor(score: f64, served: usize, id: &str) -> String {
    format!("v2:{:016x}:{served}:{id}", score.to_bits())
}

/// Decodes a cursor token, rejecting anything this index didn't produce.
fn decode_cursor(raw: &str) -> Result<CursorKey> {
    let bad = || IndexError::Cursor {
        value: raw.to_string(),
    };
    let mut parts = raw.splitn(4, ':');
    let version = parts.next().ok_or_else(bad)?;
    let bits_hex = parts.next().ok_or_else(bad)?;
    let served = parts.next().ok_or_else(bad)?;
    let id = parts.next().ok_or_else(bad)?;
    if version != "v2" || bits_hex.len() != 16 || id.is_empty() {
        return Err(bad());
    }
    let bits = u64::from_str_radix(bits_hex, 16).map_err(|_| bad())?;
    let served: usize = served.parse().map_err(|_| bad())?;
    Ok(CursorKey {
        score: f64::from_bits(bits),
        // Clamped, not trusted: `served` only sizes the candidate pools, so a
        // tampered value must not turn into an enormous `LIMIT`.
        served: served.min(MAX_CANDIDATES),
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
                Some("Paradise Golf"),
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
        assert_eq!(top.project.as_deref(), Some("Paradise Golf"));
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

        // A repeated tag is deduplicated, not counted twice. Counting it twice
        // would demand two distinct tags a note can only supply one of, and
        // silently match nothing.
        params.tags = vec!["a".to_string(), "a".to_string(), "b".to_string()];
        let deduped = index.search_notes(&params, None).unwrap();
        assert_eq!(hit_ids(&deduped), vec!["n_t1"], "duplicate tag ignored");

        params.tag_match = TagMatch::Any;
        params.tags = vec!["a".to_string(), "a".to_string()];
        let any_dup = index.search_notes(&params, None).unwrap();
        assert_eq!(
            id_set(&any_dup),
            HashSet::from(["n_t1".to_string(), "n_t2".to_string()])
        );
    }

    #[test]
    fn oversized_filter_arrays_are_rejected() {
        let index = NoteIndex::open_in_memory().unwrap();
        let mut params = query("report");
        // Distinct values, so dedup can't bring it under the cap.
        params.tags = (0..MAX_FILTER_VALUES + 1)
            .map(|i| format!("t{i}"))
            .collect();
        assert!(matches!(
            index.search_notes(&params, None),
            Err(IndexError::FilterTooLarge { field: "tags", .. })
        ));

        // Duplicates collapse below the cap and are fine.
        params.tags = vec!["a".to_string(); MAX_FILTER_VALUES + 10];
        assert!(index.search_notes(&params, None).is_ok());
    }

    // --- 10. Date bounds ----------------------------------------------------

    #[test]
    fn date_bounds_are_inclusive_and_use_the_local_calendar_day() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Local date-only.
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
        // Local calendar day 7/10, late enough that its UTC instant
        // (2026-07-11T02:00:00Z) falls on the next day. Filtering by UTC would
        // wrongly drop it.
        put(
            &mut index,
            &note(
                "n_d2",
                None,
                NoteType::Note,
                "2026-07-10T19:00:00-07:00",
                &[],
                "D2",
                "report",
            ),
        );
        // Local calendar day 7/9, whose UTC instant (2026-07-10T03:00:00Z)
        // falls inside the bound. Filtering by UTC would wrongly include it.
        put(
            &mut index,
            &note(
                "n_d3",
                None,
                NoteType::Note,
                "2026-07-09T20:00:00-07:00",
                &[],
                "D3",
                "report",
            ),
        );
        put(
            &mut index,
            &note(
                "n_d4",
                None,
                NoteType::Note,
                "2026-07-11",
                &[],
                "D4",
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
            "the user's local day, inclusive both ends"
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

    #[test]
    fn pagination_is_stable_when_a_higher_ranked_note_arrives() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        for i in 2..=6 {
            put_vec(
                &mut index,
                &note(
                    &format!("n_q{i}"),
                    None,
                    NoteType::Note,
                    "2026-07-01",
                    &[],
                    "Q",
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
        assert_eq!(hit_ids(&page1), vec!["n_q2", "n_q3"]);

        // Insert a note nearer than everything else: it takes rank 1 and pushes
        // every other note down one rank, so every RRF score changes. A cursor
        // that trusted its encoded score would land before the boundary and
        // re-serve n_q3.
        put_vec(
            &mut index,
            &note(
                "n_q0",
                None,
                NoteType::Note,
                "2026-07-01",
                &[],
                "Q0",
                "body",
            ),
            near(1),
            "chunk one",
        );

        params.cursor = page1.page.next_cursor.clone();
        let page2 = index.search_notes(&params, Some(&embedder)).unwrap();
        assert_eq!(
            hit_ids(&page2),
            vec!["n_q4", "n_q5"],
            "resumes after the boundary note, no repeat"
        );
    }

    #[test]
    fn pagination_survives_the_vector_arm_dropping_out_mid_walk() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Notes carrying both the FTS term and a vector, so losing the vector
        // arm halves every score.
        for i in 1..=5 {
            put_vec(
                &mut index,
                &note(
                    &format!("n_w{i}"),
                    None,
                    NoteType::Note,
                    "2026-07-01",
                    &[],
                    "W",
                    "alpha",
                ),
                near(i),
                "alpha",
            );
        }

        let embedder = FixedEmbedder(axis(0));
        let mut params = query("alpha");
        params.limit = 2;
        let page1 = index.search_notes(&params, Some(&embedder)).unwrap();
        let served = hit_ids(&page1);
        assert_eq!(served.len(), 2);

        // The embedder breaks between pages: page 2 fuses FTS-only, so every
        // score differs from the one the cursor was minted against.
        params.cursor = page1.page.next_cursor.clone();
        let page2 = index.search_notes(&params, Some(&FailingEmbedder)).unwrap();
        assert!(
            page2.hits.iter().all(|hit| !served.contains(&hit.id)),
            "no page-1 row re-served after the arm dropped out"
        );
    }

    // --- 15. Cursor tampering -----------------------------------------------

    #[test]
    fn tampered_cursor_is_an_error() {
        let index = NoteIndex::open_in_memory().unwrap();
        for bad in [
            "garbage",
            "v1:0000000000000000:0:n_x",
            "v3:0000000000000000:0:n_x",
            "v2:xyz:0:n_x",
            "v2:00:0:n_x",
            "v2:0000000000000000:0:",
            "v2:0000000000000000:n_x",
            "v2:0000000000000000:notanumber:n_x",
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

    #[test]
    fn pagination_walks_past_the_candidate_headroom() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // More matches than one candidate pool holds, so a fixed-depth pool
        // would report has_more: false partway through and the caller would
        // conclude it had seen every match.
        let corpus = CANDIDATE_HEADROOM + 50;
        for i in 0..corpus {
            put(
                &mut index,
                &note(
                    &format!("n_h{i:05}"),
                    None,
                    NoteType::Note,
                    "2026-07-01",
                    &[],
                    "H",
                    "report",
                ),
            );
        }

        let mut params = query("report");
        params.limit = 50;

        // The first page can't know the true total: its pool came back full.
        let page1 = index.search_notes(&params, None).unwrap();
        assert!(page1.page.has_more);
        assert!(
            page1.page.total_estimate.is_none(),
            "a truncated pool reports an unknown total, not its own size"
        );

        let mut seen = Vec::new();
        let mut page = page1;
        loop {
            seen.extend(hit_ids(&page));
            match page.page.next_cursor.clone() {
                Some(cursor) => params.cursor = Some(cursor),
                None => break,
            }
            page = index.search_notes(&params, None).unwrap();
        }

        assert_eq!(seen.len(), corpus, "every match served exactly once");
        assert_eq!(
            seen.iter().collect::<HashSet<_>>().len(),
            corpus,
            "no duplicates"
        );
        // The final page's pool outran the corpus, so the count is exact again.
        assert_eq!(page.page.total_estimate, Some(corpus as u64));
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
