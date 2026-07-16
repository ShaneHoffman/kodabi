//! The read/write query surface over the note index: upsert a note, and fetch
//! notes by their stable `id` or by `project`.

use std::collections::HashMap;

use rusqlite::{params, params_from_iter, OptionalExtension, Row};

use super::note::{normalize_date_to_utc, IndexedNote, NoteRow};
use super::{NoteIndex, Result};

/// The columns [`map_row`] reads, in order — shared by the by-id and by-project
/// queries so the two stay in lockstep.
const NOTE_COLUMNS: &str =
    "id, path, title, type, project, date_raw, date_utc, source, confidence, body";

impl NoteIndex {
    /// Inserts `note`, or updates the existing row with the same `id` in place.
    ///
    /// The `id` is the stable key, so a re-index of a moved or edited note
    /// updates the same row and preserves its `pk`. The FTS index is kept in
    /// sync by the schema's triggers, and the tag set is refreshed explicitly.
    /// The `notes_vec` embedding is *derived* content that the embedding
    /// pipeline owns (nothing is written to it here); because it goes stale when
    /// the title or body changes, an upsert that changes either drops the note's
    /// vector row so the pipeline recomputes it — a pure move keeps it. All of
    /// it is one transaction, so a partially-applied upsert is impossible.
    pub fn upsert_note(&mut self, note: &IndexedNote) -> Result<()> {
        let date_utc = normalize_date_to_utc(&note.date)?;

        let tx = self.conn.transaction()?;

        // The embedding is derived from title + body, so capture the pre-upsert
        // content to detect whether a stale vector must be dropped below.
        let previous_content: Option<(String, String)> = tx
            .query_row(
                "SELECT title, body FROM notes WHERE id = ?1",
                [&note.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        tx.execute(
            "INSERT INTO notes
                 (id, path, title, type, project, date_raw, date_utc, source, confidence, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                 path       = excluded.path,
                 title      = excluded.title,
                 type       = excluded.type,
                 project    = excluded.project,
                 date_raw   = excluded.date_raw,
                 date_utc   = excluded.date_utc,
                 source     = excluded.source,
                 confidence = excluded.confidence,
                 body       = excluded.body",
            params![
                note.id,
                note.path,
                note.title,
                note.note_type,
                note.project,
                note.date,
                date_utc,
                note.source,
                note.confidence,
                note.body,
            ],
        )?;

        let pk: i64 = tx.query_row("SELECT pk FROM notes WHERE id = ?1", [&note.id], |row| {
            row.get(0)
        })?;

        // If the embeddable content changed, the cached embedding (if the
        // pipeline has written one) no longer matches — drop it so it is
        // recomputed rather than served stale. Keyed by the stable `id`.
        if let Some((old_title, old_body)) = previous_content {
            if old_title != note.title || old_body != note.body {
                tx.execute("DELETE FROM notes_vec WHERE note_id = ?1", [&note.id])?;
            }
        }
        // Replace the tag set wholesale so a re-index reflects removed tags too.
        tx.execute("DELETE FROM note_tags WHERE note_pk = ?1", [pk])?;
        {
            let mut insert_tag =
                tx.prepare("INSERT OR IGNORE INTO note_tags (note_pk, tag) VALUES (?1, ?2)")?;
            for tag in &note.tags {
                insert_tag.execute(params![pk, tag])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Fetches the note with this `id`, or `None` if it isn't indexed.
    pub fn get_note(&self, id: &str) -> Result<Option<NoteRow>> {
        let row = self
            .conn
            .query_row(
                &format!("SELECT {NOTE_COLUMNS} FROM notes WHERE id = ?1"),
                [id],
                map_row,
            )
            .optional()?;

        match row {
            Some(mut note) => {
                note.tags = self.load_tags(&note.id)?;
                Ok(Some(note))
            }
            None => Ok(None),
        }
    }

    /// Lists the notes in `project` (or the unfiled/Inbox notes when `None`),
    /// most recent first by UTC-normalized date.
    pub fn notes_by_project(&self, project: Option<&str>) -> Result<Vec<NoteRow>> {
        // `project = ?` never matches NULL, so unfiled notes need `IS NULL` —
        // and that branch binds no parameter at all.
        let filter = if project.is_some() {
            "project = ?1"
        } else {
            "project IS NULL"
        };
        let sql =
            format!("SELECT {NOTE_COLUMNS} FROM notes WHERE {filter} ORDER BY date_utc DESC, id");

        let mut stmt = self.conn.prepare(&sql)?;
        let mut notes = match project {
            Some(p) => stmt
                .query_map([p], map_row)?
                .collect::<rusqlite::Result<Vec<NoteRow>>>()?,
            None => stmt
                .query_map([], map_row)?
                .collect::<rusqlite::Result<Vec<NoteRow>>>()?,
        };

        // Load every note's tags in a single query, then distribute — avoids an
        // N+1 round-trip per returned note.
        let ids: Vec<&str> = notes.iter().map(|n| n.id.as_str()).collect();
        let mut tags_by_id = self.load_tags_by_ids(&ids)?;
        for note in &mut notes {
            note.tags = tags_by_id.remove(&note.id).unwrap_or_default();
        }
        Ok(notes)
    }

    /// Loads one note's tags, sorted for deterministic output.
    fn load_tags(&self, id: &str) -> Result<Vec<String>> {
        // Cached: `get_note` reuses this statement across calls in a rebuild.
        let mut stmt = self.conn.prepare_cached(
            "SELECT tag FROM note_tags
             JOIN notes ON notes.pk = note_tags.note_pk
             WHERE notes.id = ?1 ORDER BY tag",
        )?;
        let tags = stmt
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(tags)
    }

    /// Loads the tags for many notes at once, keyed by note `id` and sorted
    /// within each note. Ids absent from the map (or the whole map, for an empty
    /// input) simply have no tags.
    fn load_tags_by_ids(&self, ids: &[&str]) -> Result<HashMap<String, Vec<String>>> {
        let mut tags_by_id: HashMap<String, Vec<String>> = HashMap::new();
        if ids.is_empty() {
            return Ok(tags_by_id);
        }

        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT notes.id, note_tags.tag FROM note_tags
             JOIN notes ON notes.pk = note_tags.note_pk
             WHERE notes.id IN ({placeholders}) ORDER BY notes.id, note_tags.tag"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(ids.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, tag) = row?;
            tags_by_id.entry(id).or_default().push(tag);
        }
        Ok(tags_by_id)
    }
}

/// Builds a [`NoteRow`] from a `SELECT` of [`NOTE_COLUMNS`]. `tags` are filled
/// separately by the caller.
fn map_row(row: &Row<'_>) -> rusqlite::Result<NoteRow> {
    Ok(NoteRow {
        id: row.get("id")?,
        path: row.get("path")?,
        title: row.get("title")?,
        note_type: row.get("type")?,
        project: row.get("project")?,
        date: row.get("date_raw")?,
        date_utc: row.get("date_utc")?,
        tags: Vec::new(),
        source: row.get("source")?,
        confidence: row.get("confidence")?,
        body: row.get("body")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{NoteIndex, NoteType};

    fn note(id: &str, project: Option<&str>, date: &str) -> IndexedNote {
        IndexedNote {
            id: id.to_string(),
            path: format!("{}/{id}.md", project.unwrap_or("Inbox")),
            title: format!("Title {id}"),
            note_type: NoteType::Meeting,
            project: project.map(str::to_string),
            date: date.to_string(),
            tags: vec![],
            source: "transcript".to_string(),
            confidence: Some(0.9),
            body: format!("body of {id}"),
        }
    }

    fn fts_ids(index: &NoteIndex, query: &str) -> Vec<String> {
        let mut stmt = index
            .conn
            .prepare("SELECT id FROM notes_fts JOIN notes ON notes.pk = notes_fts.rowid WHERE notes_fts MATCH ?1 ORDER BY id")
            .unwrap();
        stmt.query_map([query], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
    }

    #[test]
    fn upsert_then_get_round_trips_every_field() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut input = note(
            "n_abc123",
            Some("Paradise Golf"),
            "2026-07-09T14:00:00-07:00",
        );
        input.tags = vec!["budgeting".to_string(), "phase-2".to_string()];
        index.upsert_note(&input).unwrap();

        let got = index.get_note("n_abc123").unwrap().unwrap();
        assert_eq!(got.id, "n_abc123");
        assert_eq!(got.project.as_deref(), Some("Paradise Golf"));
        assert_eq!(got.note_type, NoteType::Meeting);
        assert_eq!(got.date, "2026-07-09T14:00:00-07:00");
        assert_eq!(got.date_utc, "2026-07-09T21:00:00Z");
        assert_eq!(got.confidence, Some(0.9));
        assert_eq!(got.tags, vec!["budgeting", "phase-2"]);
        assert_eq!(got.body, "body of n_abc123");
    }

    #[test]
    fn get_missing_note_is_none() {
        let index = NoteIndex::open_in_memory().unwrap();
        assert!(index.get_note("n_nope00").unwrap().is_none());
    }

    #[test]
    fn upsert_updates_in_place_and_resyncs_fts_and_tags() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut first = note("n_edit01", Some("Growth"), "2026-07-10");
        first.body = "aardvark opening".to_string();
        first.tags = vec!["old-tag".to_string()];
        index.upsert_note(&first).unwrap();

        let mut second = first.clone();
        second.body = "beluga rewrite".to_string();
        second.title = "Renamed".to_string();
        second.tags = vec!["new-tag".to_string()];
        index.upsert_note(&second).unwrap();

        // Still exactly one row.
        let count: i64 = index
            .conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        let got = index.get_note("n_edit01").unwrap().unwrap();
        assert_eq!(got.title, "Renamed");
        assert_eq!(got.body, "beluga rewrite");
        assert_eq!(got.tags, vec!["new-tag"]);

        // FTS reflects the new body, not the old one.
        assert_eq!(fts_ids(&index, "beluga"), vec!["n_edit01"]);
        assert!(fts_ids(&index, "aardvark").is_empty());
    }

    #[test]
    fn notes_by_project_filters_and_orders_by_utc_across_offsets() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // Raw strings would sort b before a (…14…-07:00 < …15…+00:00), but a is
        // the later instant (21:00Z > 15:00Z), so date_utc ordering puts a first.
        index
            .upsert_note(&note("n_aaa111", Some("Acme"), "2026-07-09T14:00:00-07:00"))
            .unwrap();
        index
            .upsert_note(&note("n_bbb222", Some("Acme"), "2026-07-09T15:00:00+00:00"))
            .unwrap();
        // A note in another project must not appear.
        index
            .upsert_note(&note("n_ccc333", Some("Other"), "2026-07-20"))
            .unwrap();

        let acme = index.notes_by_project(Some("Acme")).unwrap();
        let ids: Vec<&str> = acme.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["n_aaa111", "n_bbb222"]);
    }

    #[test]
    fn unfiled_notes_are_stored_and_queried_as_null_project() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&note("n_inbox1", None, "2026-07-11"))
            .unwrap();
        index
            .upsert_note(&note("n_filed1", Some("Acme"), "2026-07-11"))
            .unwrap();

        let inbox = index.notes_by_project(None).unwrap();
        let ids: Vec<&str> = inbox.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, vec!["n_inbox1"]);
        assert_eq!(inbox[0].project, None);
    }

    #[test]
    fn confidence_is_nullable_and_survives_round_trip() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut handfiled = note("n_hand01", Some("Acme"), "2026-07-11");
        handfiled.confidence = None;
        index.upsert_note(&handfiled).unwrap();

        assert_eq!(
            index.get_note("n_hand01").unwrap().unwrap().confidence,
            None
        );
    }

    #[test]
    fn an_out_of_range_confidence_is_rejected_by_the_check() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut bad = note("n_bad001", Some("Acme"), "2026-07-11");
        bad.confidence = Some(1.5);
        assert!(index.upsert_note(&bad).is_err());
    }

    #[test]
    fn empty_tags_produce_no_rows() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&note("n_notag1", Some("Acme"), "2026-07-11"))
            .unwrap();
        assert!(index.get_note("n_notag1").unwrap().unwrap().tags.is_empty());
    }

    #[test]
    fn re_upserting_with_a_new_project_relocates_the_note() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&note("n_move01", Some("Acme"), "2026-07-11"))
            .unwrap();

        // Re-index the same id under a different project.
        index
            .upsert_note(&note("n_move01", Some("Growth"), "2026-07-11"))
            .unwrap();

        let acme_ids: Vec<String> = index
            .notes_by_project(Some("Acme"))
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert!(acme_ids.is_empty(), "note should have left the old project");

        let growth_ids: Vec<String> = index
            .notes_by_project(Some("Growth"))
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(growth_ids, vec!["n_move01"]);
    }

    #[test]
    fn upsert_rejects_an_unparseable_date() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let bad = note("n_baddat", Some("Acme"), "not-a-date");
        assert!(matches!(
            index.upsert_note(&bad),
            Err(crate::index::IndexError::Date { .. })
        ));
        // The failed upsert wrote nothing.
        assert!(index.get_note("n_baddat").unwrap().is_none());
    }

    #[test]
    fn changing_the_body_drops_a_stale_embedding_but_a_move_keeps_it() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut original = note("n_embed1", Some("Acme"), "2026-07-11");
        original.body = "first body".to_string();
        index.upsert_note(&original).unwrap();

        // Stand in for the (future) embedding pipeline: write a vector keyed by
        // the note id.
        let embedding = format!("[{}]", vec!["0"; crate::index::EMBEDDING_DIM].join(","));
        index
            .conn
            .execute(
                "INSERT INTO notes_vec (note_id, embedding) VALUES ('n_embed1', ?1)",
                [&embedding],
            )
            .unwrap();
        let vec_rows = |idx: &NoteIndex| -> i64 {
            idx.conn
                .query_row(
                    "SELECT count(*) FROM notes_vec WHERE note_id = 'n_embed1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(vec_rows(&index), 1);

        // A pure move (path/project change, same title+body) keeps the vector.
        let mut moved = original.clone();
        moved.project = Some("Growth".to_string());
        moved.path = "Growth/n_embed1.md".to_string();
        index.upsert_note(&moved).unwrap();
        assert_eq!(vec_rows(&index), 1, "a move must not drop the embedding");

        // Editing the body drops the now-stale vector.
        let mut edited = moved.clone();
        edited.body = "rewritten body".to_string();
        index.upsert_note(&edited).unwrap();
        assert_eq!(vec_rows(&index), 0, "an edit must drop the stale embedding");
    }
}
