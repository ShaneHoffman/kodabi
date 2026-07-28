//! The read/write query surface over the note index: upsert a note, and fetch
//! notes by their stable `id` or by `project`.

use std::collections::HashMap;

use rusqlite::{params, params_from_iter, OptionalExtension, Row};

use super::note::{
    normalize_date_to_utc, ActionItemRow, IndexedNote, MeetingFactsRow, NoteRow, NoteType,
    NoteTypeCounts,
};
use super::scope::ProjectScope;
use super::{NoteIndex, Result};
use crate::meeting::MeetingFacts;

/// The columns [`map_row`] reads, in order — shared by the by-id and by-project
/// queries so the two stay in lockstep.
///
/// Hybrid search (`super::search`) deliberately does *not* use this list: a
/// `SearchHit` carries no body, so hydrating a page through these columns would
/// read every hit's full text only to drop it. That surface has its own
/// narrower list.
const NOTE_COLUMNS: &str =
    "id, path, title, type, project, date_raw, date_utc, source, confidence, body";

impl NoteIndex {
    /// Inserts `note`, or updates the existing row with the same `id` in place.
    ///
    /// The `id` is the stable key, so a re-index of a moved or edited note
    /// updates the same row and preserves its `pk`. The FTS index is kept in
    /// sync by the schema's triggers, and the tag set is refreshed explicitly.
    /// The `notes_vec` chunk vectors and their `note_chunks` text are *derived*
    /// content that the embedding pipeline owns (nothing is written to them
    /// here); because they go stale when the title or body changes, an upsert
    /// that changes either drops the note's chunk rows from both tables so the
    /// pipeline recomputes them — a pure move keeps them. All of it is one
    /// transaction, so a partially-applied upsert is impossible.
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

        // If the embeddable content changed, the cached chunk vectors (if the
        // pipeline has written any) no longer match — drop them from both the
        // vector table and its text companion so they are recomputed rather
        // than served stale. Keyed by the stable `id`; `note_id` is a `vec0`
        // metadata column, so the `notes_vec` delete needs no key parsing.
        if let Some((old_title, old_body)) = previous_content {
            if old_title != note.title || old_body != note.body {
                tx.execute("DELETE FROM notes_vec WHERE note_id = ?1", [&note.id])?;
                tx.execute("DELETE FROM note_chunks WHERE note_id = ?1", [&note.id])?;
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
        // Replace the meeting facts wholesale, mirroring the tag set: they are a
        // derived cache of the body + session file, and `None` (a non-meeting
        // note, or one whose facts the caller did not derive) clears any prior
        // rows.
        write_meeting_facts(&tx, &note.id, note.meeting.as_ref())?;
        tx.commit()?;
        Ok(())
    }

    /// Writes (or clears, with `None`) a note's meeting facts in its own
    /// transaction. The meeting-facts backfill's entry point: it derives facts
    /// for an already-indexed meeting note (`note_ids_missing_meeting_facts`)
    /// without re-running a full [`upsert_note`](NoteIndex::upsert_note).
    pub fn set_meeting_facts(&mut self, note_id: &str, facts: Option<&MeetingFacts>) -> Result<()> {
        let tx = self.conn.transaction()?;
        write_meeting_facts(&tx, note_id, facts)?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes the note with this `id`, returning whether a row existed.
    ///
    /// The reconcile pass calls this for ids whose file has left the vault. The
    /// `notes_ad` trigger clears the FTS row and the `note_tags` foreign key
    /// cascades, but `notes_vec`/`note_chunks` and the meeting-facts tables
    /// (`note_meetings`/`note_decisions`/`note_action_items`) are *not* keyed to
    /// `notes`, so their rows are cleared explicitly (by the `note_id` column) in
    /// the same transaction — a partial delete is impossible.
    pub fn delete_note(&mut self, id: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM notes_vec WHERE note_id = ?1", [id])?;
        tx.execute("DELETE FROM note_chunks WHERE note_id = ?1", [id])?;
        tx.execute("DELETE FROM note_meetings WHERE note_id = ?1", [id])?;
        tx.execute("DELETE FROM note_decisions WHERE note_id = ?1", [id])?;
        tx.execute("DELETE FROM note_action_items WHERE note_id = ?1", [id])?;
        let removed = tx.execute("DELETE FROM notes WHERE id = ?1", [id])?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Every indexed `(id, path)` pair, ordered by `id` — the reconcile pass's
    /// stale-diff input: any id here but absent from a full disk scan is a
    /// candidate for deletion, and its stored `path` decides whether a
    /// mid-write file earns a reprieve.
    pub fn note_ids_and_paths(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path FROM notes ORDER BY id")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
        Ok(rows)
    }

    /// Empties the whole index — notes, tags, FTS, vectors, chunk text, and
    /// meeting facts — so a rebuild can repopulate it from files alone. Truncates
    /// the tables in one transaction; it never deletes or reopens the database
    /// file (WAL and Windows file locks make that fragile, and the schema is
    /// already correct).
    pub fn clear(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM notes_vec", [])?;
        tx.execute("DELETE FROM note_chunks", [])?;
        tx.execute("DELETE FROM note_meetings", [])?;
        tx.execute("DELETE FROM note_decisions", [])?;
        tx.execute("DELETE FROM note_action_items", [])?;
        // Clearing `notes` fires `notes_ad` per row (FTS) and cascades tags.
        tx.execute("DELETE FROM notes", [])?;
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

    /// The most recent notes in `scope`, newest first by UTC-normalized date
    /// with an id tiebreak. The scope-aware, limited generalization of
    /// [`notes_by_project`](Self::notes_by_project), for
    /// `get_project_context`'s `recent_notes` section.
    ///
    /// A `limit` of 0 is an empty result, matching the schema's "0 to omit".
    pub fn recent_notes(&self, scope: &ProjectScope, limit: u32) -> Result<Vec<NoteRow>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let (filter, values) = match scope.predicate() {
            Some((clause, values)) => (format!("WHERE {clause}"), values),
            None => (String::new(), Vec::new()),
        };
        let sql = format!(
            "SELECT {NOTE_COLUMNS} FROM notes {filter} ORDER BY date_utc DESC, id LIMIT {limit}"
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let mut notes = stmt
            .query_map(params_from_iter(values), map_row)?
            .collect::<rusqlite::Result<Vec<NoteRow>>>()?;

        let ids: Vec<&str> = notes.iter().map(|n| n.id.as_str()).collect();
        let mut tags_by_id = self.load_tags_by_ids(&ids)?;
        for note in &mut notes {
            note.tags = tags_by_id.remove(&note.id).unwrap_or_default();
        }
        Ok(notes)
    }

    /// Note counts in `scope`, one per [`NoteType`] (zero for absent types).
    pub fn note_counts_by_type(&self, scope: &ProjectScope) -> Result<NoteTypeCounts> {
        let (filter, values) = match scope.predicate() {
            Some((clause, values)) => (format!("WHERE {clause}"), values),
            None => (String::new(), Vec::new()),
        };
        let sql = format!("SELECT type, count(*) FROM notes {filter} GROUP BY type");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, i64)>>>()?;

        let mut counts = NoteTypeCounts::default();
        for (note_type, count) in rows {
            let count = count as u32;
            // The `type` column is CHECK-constrained to the three values, so an
            // unknown one cannot appear; ignore rather than fail if it somehow does.
            match note_type.parse::<NoteType>() {
                Ok(NoteType::Meeting) => counts.meeting = count,
                Ok(NoteType::Note) => counts.note = count,
                Ok(NoteType::Chat) => counts.chat = count,
                Err(_) => {}
            }
        }
        Ok(counts)
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
    ///
    /// Shared with `super::search`, which hydrates a search page's tags the same
    /// batched way `notes_by_project` does.
    pub(super) fn load_tags_by_ids(&self, ids: &[&str]) -> Result<HashMap<String, Vec<String>>> {
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

    /// Fetches a meeting note's scalar facts (`duration_seconds`,
    /// `speaker_count`) plus its ordered decisions, or `None` when the note has
    /// no `note_meetings` row — a non-meeting note, or a meeting note not yet
    /// backfilled after the v3 migration. The action items are a separate read
    /// ([`get_action_items`](NoteIndex::get_action_items)), so `search` and
    /// `notes_by_project` never join these tables.
    pub fn get_meeting_facts(&self, id: &str) -> Result<Option<MeetingFactsRow>> {
        let scalars = self
            .conn
            .query_row(
                "SELECT duration_seconds, speaker_count FROM note_meetings WHERE note_id = ?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?.map(|v| v as u32),
                        row.get::<_, Option<i64>>(1)?.map(|v| v as u32),
                    ))
                },
            )
            .optional()?;
        let Some((duration_seconds, speaker_count)) = scalars else {
            return Ok(None);
        };

        let mut stmt = self
            .conn
            .prepare_cached("SELECT text FROM note_decisions WHERE note_id = ?1 ORDER BY seq")?;
        let decisions = stmt
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        Ok(Some(MeetingFactsRow {
            duration_seconds,
            speaker_count,
            decisions,
        }))
    }

    /// Fetches a note's action items in body order (empty when none). `overdue`
    /// is not stored — the caller derives it from `done` + `due_date`.
    pub fn get_action_items(&self, id: &str) -> Result<Vec<ActionItemRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT item_id, description, owner, due_date, done, extracted_date
             FROM note_action_items WHERE note_id = ?1 ORDER BY seq",
        )?;
        let items = stmt
            .query_map([id], |row| {
                Ok(ActionItemRow {
                    id: row.get("item_id")?,
                    description: row.get("description")?,
                    owner: row.get("owner")?,
                    due_date: row.get("due_date")?,
                    done: row.get("done")?,
                    extracted_date: row.get("extracted_date")?,
                })
            })?
            .collect::<rusqlite::Result<Vec<ActionItemRow>>>()?;
        Ok(items)
    }

    /// Every meeting note id with no `note_meetings` row yet, ordered by `id` —
    /// the meeting-facts backfill's work list, mirroring
    /// [`note_ids_missing_embeddings`](NoteIndex::note_ids_missing_embeddings).
    /// The v3 migration adds the meeting tables empty, so every existing meeting
    /// note appears here until the backfill derives its facts.
    pub fn note_ids_missing_meeting_facts(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM notes
             WHERE type = 'meeting'
               AND id NOT IN (SELECT note_id FROM note_meetings)
             ORDER BY id",
        )?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(ids)
    }
}

/// Replaces a note's meeting-facts rows across all three tables in the caller's
/// transaction. `None` clears them without inserting — a non-meeting note, or a
/// note whose facts were not derived — mirroring how the tag set is replaced
/// wholesale so a re-index reflects removed decisions/items too.
fn write_meeting_facts(
    tx: &rusqlite::Transaction<'_>,
    note_id: &str,
    facts: Option<&MeetingFacts>,
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM note_meetings WHERE note_id = ?1", [note_id])?;
    tx.execute("DELETE FROM note_decisions WHERE note_id = ?1", [note_id])?;
    tx.execute(
        "DELETE FROM note_action_items WHERE note_id = ?1",
        [note_id],
    )?;

    let Some(facts) = facts else {
        return Ok(());
    };

    tx.execute(
        "INSERT INTO note_meetings (note_id, duration_seconds, speaker_count)
         VALUES (?1, ?2, ?3)",
        params![
            note_id,
            facts.duration_seconds.map(i64::from),
            facts.speaker_count.map(i64::from),
        ],
    )?;
    {
        let mut insert_decision =
            tx.prepare("INSERT INTO note_decisions (note_id, seq, text) VALUES (?1, ?2, ?3)")?;
        for (seq, text) in facts.decisions.iter().enumerate() {
            insert_decision.execute(params![note_id, seq as i64, text])?;
        }
    }
    {
        let mut insert_item = tx.prepare(
            "INSERT INTO note_action_items
                 (note_id, seq, item_id, description, owner, due_date, done, extracted_date)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (seq, item) in facts.action_items.iter().enumerate() {
            insert_item.execute(params![
                note_id,
                seq as i64,
                item.id,
                item.description,
                item.owner,
                item.due_date,
                item.done,
                item.extracted_date,
            ])?;
        }
    }
    Ok(())
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
    use crate::meeting::ActionItemFact;

    /// A couple of decisions and two action items (one open+due, one done) for
    /// the meeting-facts tests.
    fn meeting_facts() -> MeetingFacts {
        MeetingFacts {
            duration_seconds: Some(1800),
            speaker_count: Some(2),
            decisions: vec!["Ship it".to_string(), "Freeze the API".to_string()],
            action_items: vec![
                ActionItemFact {
                    id: "a_send01".to_string(),
                    description: "send the memo".to_string(),
                    owner: "Jane".to_string(),
                    due_date: Some("2026-07-15".to_string()),
                    done: false,
                    extracted_date: Some("2026-07-10".to_string()),
                },
                ActionItemFact {
                    id: "a_book02".to_string(),
                    description: "book the room".to_string(),
                    owner: "Unassigned".to_string(),
                    due_date: None,
                    done: true,
                    extracted_date: Some("2026-07-10".to_string()),
                },
            ],
        }
    }

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
            meeting: None,
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
            Some("Briarwood Golf"),
            "2026-07-09T14:00:00-07:00",
        );
        input.tags = vec!["budgeting".to_string(), "phase-2".to_string()];
        index.upsert_note(&input).unwrap();

        let got = index.get_note("n_abc123").unwrap().unwrap();
        assert_eq!(got.id, "n_abc123");
        assert_eq!(got.project.as_deref(), Some("Briarwood Golf"));
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
    fn meeting_facts_round_trip_through_upsert() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut input = note("n_meet01", Some("Growth"), "2026-07-10");
        input.meeting = Some(meeting_facts());
        index.upsert_note(&input).unwrap();

        let scalars = index.get_meeting_facts("n_meet01").unwrap().unwrap();
        assert_eq!(scalars.duration_seconds, Some(1800));
        assert_eq!(scalars.speaker_count, Some(2));
        assert_eq!(scalars.decisions, vec!["Ship it", "Freeze the API"]);

        let items = index.get_action_items("n_meet01").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "a_send01");
        assert_eq!(items[0].owner, "Jane");
        assert_eq!(items[0].due_date.as_deref(), Some("2026-07-15"));
        assert!(!items[0].done);
        assert_eq!(items[1].id, "a_book02");
        assert!(items[1].done);
        assert_eq!(items[1].due_date, None);
    }

    #[test]
    fn a_note_with_no_meeting_facts_reads_back_none() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // A meeting note upserted without derived facts.
        index
            .upsert_note(&note("n_bare01", Some("Growth"), "2026-07-10"))
            .unwrap();
        assert!(index.get_meeting_facts("n_bare01").unwrap().is_none());
        assert!(index.get_action_items("n_bare01").unwrap().is_empty());
    }

    #[test]
    fn re_upsert_replaces_meeting_facts_wholesale() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut first = note("n_meet01", Some("Growth"), "2026-07-10");
        first.meeting = Some(meeting_facts());
        index.upsert_note(&first).unwrap();

        // Re-index the same note with emptied facts: the prior rows must go.
        let mut second = first.clone();
        second.meeting = Some(MeetingFacts {
            duration_seconds: None,
            speaker_count: None,
            decisions: Vec::new(),
            action_items: Vec::new(),
        });
        index.upsert_note(&second).unwrap();

        let scalars = index.get_meeting_facts("n_meet01").unwrap().unwrap();
        assert_eq!(scalars.duration_seconds, None);
        assert!(scalars.decisions.is_empty());
        assert!(index.get_action_items("n_meet01").unwrap().is_empty());
    }

    #[test]
    fn note_ids_missing_meeting_facts_lists_only_unbackfilled_meetings() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        // A meeting note with no facts derived yet — the backfill's target.
        index
            .upsert_note(&note("n_bare01", Some("Growth"), "2026-07-10"))
            .unwrap();
        // A meeting note that already carries facts.
        let mut filled = note("n_full01", Some("Growth"), "2026-07-10");
        filled.meeting = Some(meeting_facts());
        index.upsert_note(&filled).unwrap();
        // A non-meeting note is never in the list.
        let mut plain = note("n_note01", Some("Growth"), "2026-07-10");
        plain.note_type = NoteType::Note;
        index.upsert_note(&plain).unwrap();

        assert_eq!(
            index.note_ids_missing_meeting_facts().unwrap(),
            vec!["n_bare01".to_string()]
        );
    }

    #[test]
    fn delete_and_clear_remove_meeting_facts() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut input = note("n_meet01", Some("Growth"), "2026-07-10");
        input.meeting = Some(meeting_facts());
        index.upsert_note(&input).unwrap();

        index.delete_note("n_meet01").unwrap();
        assert!(index.get_meeting_facts("n_meet01").unwrap().is_none());
        assert!(index.get_action_items("n_meet01").unwrap().is_empty());

        // `clear` empties the meeting-facts tables too.
        let mut again = note("n_meet02", Some("Growth"), "2026-07-10");
        again.meeting = Some(meeting_facts());
        index.upsert_note(&again).unwrap();
        index.clear().unwrap();
        assert!(index.get_meeting_facts("n_meet02").unwrap().is_none());
        let remaining: i64 = index
            .conn
            .query_row("SELECT count(*) FROM note_action_items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
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
    fn changing_the_body_drops_stale_chunks_but_a_move_keeps_them() {
        use crate::index::{EmbeddedChunk, EMBEDDING_DIM};

        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut original = note("n_embed1", Some("Acme"), "2026-07-11");
        original.body = "first body".to_string();
        index.upsert_note(&original).unwrap();

        // Stand in for the embedding pipeline: write two chunk vectors (and
        // their text) keyed by the note id, through the real store API.
        let chunks = vec![
            EmbeddedChunk {
                text: "first body".to_string(),
                embedding: vec![0.0; EMBEDDING_DIM],
            },
            EmbeddedChunk {
                text: "more".to_string(),
                embedding: vec![0.0; EMBEDDING_DIM],
            },
        ];
        index.set_note_chunks("n_embed1", &chunks).unwrap();

        // Count rows in both the vector table and its text companion — the
        // invariant is that they move in lockstep.
        let rows = |idx: &NoteIndex| -> (i64, i64) {
            let vec_rows = idx
                .conn
                .query_row(
                    "SELECT count(*) FROM notes_vec WHERE note_id = 'n_embed1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let text_rows = idx
                .conn
                .query_row(
                    "SELECT count(*) FROM note_chunks WHERE note_id = 'n_embed1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            (vec_rows, text_rows)
        };
        assert_eq!(rows(&index), (2, 2));

        // A pure move (path/project change, same title+body) keeps the chunks.
        let mut moved = original.clone();
        moved.project = Some("Growth".to_string());
        moved.path = "Growth/n_embed1.md".to_string();
        index.upsert_note(&moved).unwrap();
        assert_eq!(rows(&index), (2, 2), "a move must not drop the chunks");

        // Editing the body drops the now-stale chunks from both tables.
        let mut edited = moved.clone();
        edited.body = "rewritten body".to_string();
        index.upsert_note(&edited).unwrap();
        assert_eq!(rows(&index), (0, 0), "an edit must drop the stale chunks");

        // A title change alone also invalidates (the title is prepended to
        // every embedded chunk).
        index.set_note_chunks("n_embed1", &chunks).unwrap();
        assert_eq!(rows(&index), (2, 2));
        let mut retitled = edited.clone();
        retitled.title = "A New Title".to_string();
        index.upsert_note(&retitled).unwrap();
        assert_eq!(rows(&index), (0, 0), "a title change must drop the chunks");
    }

    #[test]
    fn delete_note_removes_the_row_fts_tags_and_chunks() {
        use crate::index::{EmbeddedChunk, EMBEDDING_DIM};

        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut input = note("n_del001", Some("Acme"), "2026-07-11");
        input.body = "walrus body".to_string();
        input.tags = vec!["a-tag".to_string()];
        index.upsert_note(&input).unwrap();
        index
            .set_note_chunks(
                "n_del001",
                &[EmbeddedChunk {
                    text: "walrus body".to_string(),
                    embedding: vec![0.0; EMBEDDING_DIM],
                }],
            )
            .unwrap();

        assert!(index.delete_note("n_del001").unwrap());

        // The row, its FTS entry, its tags, and its chunk rows are all gone.
        assert!(index.get_note("n_del001").unwrap().is_none());
        assert!(fts_ids(&index, "walrus").is_empty());
        let tag_rows: i64 = index
            .conn
            .query_row("SELECT count(*) FROM note_tags", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tag_rows, 0);
        let vec_rows: i64 = index
            .conn
            .query_row(
                "SELECT count(*) FROM notes_vec WHERE note_id = 'n_del001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let text_rows: i64 = index
            .conn
            .query_row(
                "SELECT count(*) FROM note_chunks WHERE note_id = 'n_del001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((vec_rows, text_rows), (0, 0));
    }

    #[test]
    fn delete_of_an_unknown_id_reports_no_row() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        assert!(!index.delete_note("n_ghost0").unwrap());
    }

    #[test]
    fn note_ids_and_paths_lists_every_row_ordered_by_id() {
        let mut index = NoteIndex::open_in_memory().unwrap();
        index
            .upsert_note(&note("n_zzz999", Some("Acme"), "2026-07-11"))
            .unwrap();
        index
            .upsert_note(&note("n_aaa111", None, "2026-07-11"))
            .unwrap();

        assert_eq!(
            index.note_ids_and_paths().unwrap(),
            vec![
                ("n_aaa111".to_string(), "Inbox/n_aaa111.md".to_string()),
                ("n_zzz999".to_string(), "Acme/n_zzz999.md".to_string()),
            ]
        );
    }

    #[test]
    fn clear_empties_every_table() {
        use crate::index::{EmbeddedChunk, EMBEDDING_DIM};

        let mut index = NoteIndex::open_in_memory().unwrap();
        let mut input = note("n_clr001", Some("Acme"), "2026-07-11");
        input.tags = vec!["t".to_string()];
        input.meeting = Some(meeting_facts());
        index.upsert_note(&input).unwrap();
        index
            .set_note_chunks(
                "n_clr001",
                &[EmbeddedChunk {
                    text: "body".to_string(),
                    embedding: vec![0.0; EMBEDDING_DIM],
                }],
            )
            .unwrap();

        index.clear().unwrap();

        for (table, count) in [
            ("notes", 0i64),
            ("note_tags", 0),
            ("notes_vec", 0),
            ("note_chunks", 0),
            ("note_meetings", 0),
            ("note_decisions", 0),
            ("note_action_items", 0),
        ] {
            let got: i64 = index
                .conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(got, count, "{table} should be empty after clear");
        }
        // A cleared index still works: upsert after clear round-trips.
        index
            .upsert_note(&note("n_after0", Some("Acme"), "2026-07-11"))
            .unwrap();
        assert!(index.get_note("n_after0").unwrap().is_some());
    }
}
