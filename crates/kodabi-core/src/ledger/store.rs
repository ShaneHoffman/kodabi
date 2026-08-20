//! Entry storage: the row shapes, the lifecycle state machine, evidence claims,
//! and the supersede/refresh graph.
//!
//! Every mutator takes `now` as an RFC 3339 UTC string rather than reading the
//! clock, and marks the affected project's snapshot dirty on the way out.

use std::collections::BTreeSet;

use rusqlite::{params, Connection, OptionalExtension, Row};
use unicode_normalization::UnicodeNormalization;

use super::{
    invalid_field, mint_id, ClosedVia, Direction, EnrolledVia, EntryState, EvidenceSource, Ledger,
    LedgerError, LinkKind, Result, UntrackedVia,
};

/// One tracked commitment.
///
/// # What is deliberately absent
///
/// * **`done`** — the note's checkbox owns done/not-done. A reader that needs it
///   joins the index's `note_action_items` through the entry's active
///   [`ItemRef`]s. [`EntryState::Closed`] is a *different, stronger* claim: a
///   human or a piece of evidence judged the commitment resolved, and that
///   judgement survives the line being edited away.
/// * **`due_date`** — the newest source line carries it, and caching it here
///   would create a second copy that goes stale the moment someone edits the
///   note.
///
/// # What is deliberately present
///
/// `owner` and `description` are cached verbatim (plus normalized forms) even
/// though the note also holds them. This is not a duplicated truth: Markdown
/// cannot carry *identity across its own edits*, and these fields are what make
/// the entry survivable. They are the exact-match key for re-mention linking,
/// the only render source for an entry whose source line has vanished, and the
/// input to [`Direction`]. When a live ref exists, readers should display the
/// note's current text, not this.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    pub entry_id: String,
    pub state: EntryState,
    pub direction: Direction,
    /// Last-known owner, verbatim from the grammar.
    pub owner: String,
    /// Last-known description, verbatim from the grammar.
    pub description: String,
    /// Project slug the entry currently belongs to; follows the newest mention.
    pub project: String,
    pub created_at: String,
    pub updated_at: String,
    /// The `date_utc` of the newest note that mentions this commitment. Feeds
    /// the aging surface; deliberately the note's date, not the sync's wall
    /// clock, so re-indexing an old vault does not make everything look fresh.
    pub last_mention: String,
    /// When an evidence provider last looked; `None` until one has.
    pub last_evidence_check: Option<String>,
    /// Local calendar day (`YYYY-MM-DD`) the snooze lapses. Present exactly when
    /// the state is [`EntryState::Snoozed`]; expiry is evaluated at read time.
    pub snoozed_until: Option<String>,
    /// Present exactly when the state is [`EntryState::Closed`].
    pub closed_via: Option<ClosedVia>,
    /// Why the entry needs a human; present only in [`EntryState::NeedsReview`].
    pub review_reason: Option<String>,
    /// Why this entry is in the ledger at all. Set once at creation and never
    /// rewritten by a tracking flip; a manual promote is the one exception,
    /// because it is the newest true answer to the question.
    pub enrolled_via: EnrolledVia,
    /// Present exactly when the state is [`EntryState::Untracked`].
    pub untracked_via: Option<UntrackedVia>,
    /// Whether a person has acted on this entry (closed, waived, snoozed,
    /// reopened, judged evidence, tracked, untracked).
    ///
    /// Retro-application reads this and nothing else to decide what it may
    /// override: a tracking flip is a default, and a default never overrules
    /// someone who already looked. Set by the shell's mutation path, which is
    /// the only place a human's judgement enters the ledger — the machine's
    /// sync and distill paths never route through it.
    pub touched: bool,
}

/// A link from an entry to one extracted action-item occurrence.
///
/// Retired refs are kept, never deleted: they are the audit trail of how a
/// commitment's wording drifted, and the hint that lets a restore re-attach an
/// entry whose current line was edited after the snapshot was written.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemRef {
    pub entry_id: String,
    /// The `a_` id of the extracted item. A reference, never a foreign key:
    /// the index that mints it is a disposable cache.
    pub item_id: String,
    pub note_id: String,
    pub active: bool,
    pub linked_at: String,
    pub retired_at: Option<String>,
}

/// One claim that a commitment was (or was not) fulfilled.
///
/// `confidence` is stored raw; the confidence-split machinery that decides what
/// a mixed set of claims means lives with the evidence providers, not here.
#[derive(Debug, Clone, PartialEq)]
pub struct Evidence {
    pub evidence_id: String,
    pub entry_id: String,
    pub source: EvidenceSource,
    /// URL, note id, or provider-specific reference; `None` for a bare claim.
    pub reference: Option<String>,
    pub confidence: f64,
    pub observed_at: String,
}

/// A supersede/refresh edge between two entries.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryLink {
    pub from_entry: String,
    pub to_entry: String,
    pub kind: LinkKind,
    pub created_at: String,
}

/// An entry with everything hanging off it.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryDetail {
    pub entry: LedgerEntry,
    /// Active refs first, then retired, each by `linked_at`.
    pub item_refs: Vec<ItemRef>,
    pub evidence: Vec<Evidence>,
    /// Edges where this entry is the `from` — what it supersedes or refreshes.
    pub links_out: Vec<EntryLink>,
    /// Edges where this entry is the `to` — what superseded or refreshed it.
    pub links_in: Vec<EntryLink>,
}

/// Filters for [`Ledger::list_entries`]. All conditions are ANDed; a `None`
/// field is not constrained.
#[derive(Debug, Clone, Default)]
pub struct EntryFilter {
    /// Match any of these states. `None` matches every state; `Some(vec![])`
    /// matches nothing (an explicitly empty set is a question with no answers,
    /// not an absent filter — the same rule the index's outstanding query uses).
    pub states: Option<Vec<EntryState>>,
    pub project: Option<String>,
    /// Widen `project` to its descendants (`Growth` also matches `Growth/Q3`).
    pub include_descendants: bool,
    pub direction: Option<Direction>,
    /// Entries with an *active* ref in this note.
    pub note_id: Option<String>,
    /// `last_mention` strictly before this instant — the aging surface.
    pub stale_before: Option<String>,
    /// Never checked, or last checked strictly before this instant — the
    /// evidence providers' work queue.
    pub unchecked_since: Option<String>,
}

/// Normalizes text for matching: NFC, lowercase, whitespace runs collapsed to a
/// single space, trimmed.
///
/// Shared by the stored `*_norm` columns and every match in [`super::sync`], so
/// a stored key and a freshly computed one can never disagree. NFC first so two
/// spellings of an accented name compare equal.
pub(crate) fn normalize(text: &str) -> String {
    text.nfc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The columns of `ledger_entries`, in the order [`row_to_entry`] reads them.
const ENTRY_COLUMNS: &str = "entry_id, state, direction, owner, description, project, \
     created_at, updated_at, last_mention, last_evidence_check, snoozed_until, closed_via, \
     review_reason, enrolled_via, untracked_via, touched";

/// Maps a row selecting [`ENTRY_COLUMNS`] into a [`LedgerEntry`].
fn row_to_entry(row: &Row<'_>) -> rusqlite::Result<LedgerEntry> {
    let state: String = row.get(1)?;
    let direction: String = row.get(2)?;
    let closed_via: Option<String> = row.get(11)?;
    let enrolled_via: String = row.get(13)?;
    let untracked_via: Option<String> = row.get(14)?;
    // A stored value that fails to parse means the database was written by a
    // future version or edited by hand; surface it as a SQLite-shaped error
    // rather than panicking inside the row mapper.
    let to_sqlite = |err: LedgerError| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(err))
    };
    Ok(LedgerEntry {
        entry_id: row.get(0)?,
        state: EntryState::parse(&state).map_err(to_sqlite)?,
        direction: Direction::parse(&direction).map_err(to_sqlite)?,
        owner: row.get(3)?,
        description: row.get(4)?,
        project: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        last_mention: row.get(8)?,
        last_evidence_check: row.get(9)?,
        snoozed_until: row.get(10)?,
        closed_via: closed_via
            .map(|raw| ClosedVia::parse(&raw))
            .transpose()
            .map_err(to_sqlite)?,
        review_reason: row.get(12)?,
        enrolled_via: EnrolledVia::parse(&enrolled_via).map_err(to_sqlite)?,
        untracked_via: untracked_via
            .map(|raw| UntrackedVia::parse(&raw))
            .transpose()
            .map_err(to_sqlite)?,
        touched: row.get::<_, i64>(15)? != 0,
    })
}

/// Whether a lifecycle transition is allowed.
///
/// The table, with `Superseded` reachable only through [`Ledger::link_entries`]
/// so a link and a state can never disagree:
///
/// | from → to      | Open | NeedsReview | Closed | Superseded | Waived | Snoozed | Untracked |
/// |----------------|------|-------------|--------|------------|--------|---------|-----------|
/// | Open           | yes  | yes         | yes    | via link   | yes    | yes     | yes       |
/// | NeedsReview    | yes  | yes         | yes    | via link   | yes    | no      | yes       |
/// | Snoozed        | yes  | yes         | yes    | via link   | yes    | yes     | yes       |
/// | Closed         | yes  | no          | yes    | no         | no     | no      | no        |
/// | Superseded     | via unlink | no    | no     | yes        | no     | no      | no        |
/// | Waived         | yes  | no          | no     | no         | yes    | no      | no        |
/// | Untracked      | yes  | no          | no     | no         | no     | no      | yes       |
///
/// `Untracked` is only reachable from the live states: untracking something
/// already closed or waived would overwrite a real judgement with a weaker one.
/// Getting back out is the universal `Open` row, so a mistaken untrack is undone
/// by the same Reopen every other exit uses.
fn transition_allowed(from: EntryState, to: EntryState) -> bool {
    use EntryState::*;
    if from == to {
        return true;
    }
    match to {
        // Reopen, unsnooze, or resolve a review: always available, so a human is
        // never stuck with a state the machine put them in.
        Open => true,
        NeedsReview => matches!(from, Open | Snoozed),
        Closed | Waived => matches!(from, Open | NeedsReview | Snoozed),
        Snoozed => matches!(from, Open | Snoozed),
        Superseded => matches!(from, Open | NeedsReview | Snoozed),
        Untracked => matches!(from, Open | NeedsReview | Snoozed),
    }
}

impl Ledger {
    // -----------------------------------------------------------------------
    // Reads
    // -----------------------------------------------------------------------

    /// Whether the ledger holds no entries — the restore precondition.
    pub fn is_empty(&self) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM ledger_entries", [], |row| row.get(0))?;
        Ok(count == 0)
    }

    /// Fetches one entry with its refs, evidence, and links.
    pub fn get_entry(&self, entry_id: &str) -> Result<Option<EntryDetail>> {
        let entry = self
            .conn
            .query_row(
                &format!("SELECT {ENTRY_COLUMNS} FROM ledger_entries WHERE entry_id = ?1"),
                [entry_id],
                row_to_entry,
            )
            .optional()?;
        let Some(entry) = entry else {
            return Ok(None);
        };
        Ok(Some(EntryDetail {
            item_refs: self.item_refs(entry_id)?,
            evidence: self.evidence_for(entry_id)?,
            links_out: self.links(entry_id, true)?,
            links_in: self.links(entry_id, false)?,
            entry,
        }))
    }

    /// Lists entries matching `filter`, newest mention first.
    pub fn list_entries(&self, filter: &EntryFilter) -> Result<Vec<LedgerEntry>> {
        let mut sql = format!("SELECT {ENTRY_COLUMNS} FROM ledger_entries WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(states) = &filter.states {
            if states.is_empty() {
                return Ok(Vec::new());
            }
            let slots = states
                .iter()
                .map(|state| {
                    args.push(Box::new(state.as_str()));
                    format!("?{}", args.len())
                })
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!(" AND state IN ({slots})"));
        }
        if let Some(project) = &filter.project {
            args.push(Box::new(project.clone()));
            let exact = args.len();
            if filter.include_descendants {
                // Prefix-compare rather than LIKE: a slug is user-supplied text
                // and LIKE would give `_` and `%` in it wildcard meaning.
                //
                // The length is in `char`s, not bytes: SQLite's `substr` counts
                // characters, so a byte length would over-read a slug carrying
                // any non-ASCII character and silently drop its descendants.
                let prefix = format!("{project}/");
                let prefix_chars = prefix.chars().count();
                args.push(Box::new(i64::try_from(prefix_chars).unwrap_or(i64::MAX)));
                let len = args.len();
                args.push(Box::new(prefix));
                let value = args.len();
                sql.push_str(&format!(
                    " AND (project = ?{exact} OR substr(project, 1, ?{len}) = ?{value})"
                ));
            } else {
                sql.push_str(&format!(" AND project = ?{exact}"));
            }
        }
        if let Some(direction) = filter.direction {
            args.push(Box::new(direction.as_str()));
            sql.push_str(&format!(" AND direction = ?{}", args.len()));
        }
        if let Some(note_id) = &filter.note_id {
            args.push(Box::new(note_id.clone()));
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM ledger_item_refs r
                     WHERE r.entry_id = ledger_entries.entry_id AND r.note_id = ?{}
                       AND r.active = 1)",
                args.len()
            ));
        }
        if let Some(before) = &filter.stale_before {
            args.push(Box::new(before.clone()));
            sql.push_str(&format!(" AND last_mention < ?{}", args.len()));
        }
        if let Some(before) = &filter.unchecked_since {
            args.push(Box::new(before.clone()));
            sql.push_str(&format!(
                " AND (last_evidence_check IS NULL OR last_evidence_check < ?{})",
                args.len()
            ));
        }
        // Ties broken by id so a page boundary is deterministic.
        sql.push_str(" ORDER BY last_mention DESC, entry_id");

        let mut stmt = self.conn.prepare(&sql)?;
        let params = rusqlite::params_from_iter(args.iter().map(|arg| arg.as_ref()));
        let rows = stmt.query_map(params, row_to_entry)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// [`Ledger::list_entries`], hydrated: every match with its refs, evidence,
    /// and links.
    ///
    /// The read a whole surface needs in one pass. A commitments view renders
    /// the source line through the entry's active [`ItemRef`] and the pending
    /// claims through its [`Evidence`], so fetching entries and then walking
    /// back per row would be the same query N more times.
    pub fn list_details(&self, filter: &EntryFilter) -> Result<Vec<EntryDetail>> {
        self.hydrate(self.list_entries(filter)?)
    }

    /// Every entry with an active ref in `note_id`.
    pub fn entries_for_note(&self, note_id: &str) -> Result<Vec<EntryDetail>> {
        self.hydrate(self.list_entries(&EntryFilter {
            note_id: Some(note_id.to_string()),
            ..EntryFilter::default()
        })?)
    }

    /// Attaches refs, evidence, and links to each entry, order preserved.
    fn hydrate(&self, entries: Vec<LedgerEntry>) -> Result<Vec<EntryDetail>> {
        entries
            .into_iter()
            .map(|entry| {
                Ok(EntryDetail {
                    item_refs: self.item_refs(&entry.entry_id)?,
                    evidence: self.evidence_for(&entry.entry_id)?,
                    links_out: self.links(&entry.entry_id, true)?,
                    links_in: self.links(&entry.entry_id, false)?,
                    entry,
                })
            })
            .collect()
    }

    /// The entry that currently owns one extracted line, if any.
    pub fn entry_for_item(&self, note_id: &str, item_id: &str) -> Result<Option<LedgerEntry>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {ENTRY_COLUMNS} FROM ledger_entries
                     WHERE entry_id = (SELECT entry_id FROM ledger_item_refs
                                       WHERE note_id = ?1 AND item_id = ?2 AND active = 1)"
                ),
                params![note_id, item_id],
                row_to_entry,
            )
            .optional()?)
    }

    /// All refs for an entry, active first then retired, each by `linked_at`.
    fn item_refs(&self, entry_id: &str) -> Result<Vec<ItemRef>> {
        let mut stmt = self.conn.prepare(
            "SELECT entry_id, item_id, note_id, active, linked_at, retired_at
             FROM ledger_item_refs WHERE entry_id = ?1
             ORDER BY active DESC, linked_at, item_id",
        )?;
        let rows = stmt.query_map([entry_id], |row| {
            Ok(ItemRef {
                entry_id: row.get(0)?,
                item_id: row.get(1)?,
                note_id: row.get(2)?,
                active: row.get::<_, i64>(3)? == 1,
                linked_at: row.get(4)?,
                retired_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// All evidence for an entry, oldest observation first.
    fn evidence_for(&self, entry_id: &str) -> Result<Vec<Evidence>> {
        let mut stmt = self.conn.prepare(
            "SELECT evidence_id, entry_id, source, reference, confidence, observed_at
             FROM ledger_evidence WHERE entry_id = ?1 ORDER BY observed_at, evidence_id",
        )?;
        let rows = stmt.query_map([entry_id], |row| {
            let source: String = row.get(2)?;
            Ok(Evidence {
                evidence_id: row.get(0)?,
                entry_id: row.get(1)?,
                source: EvidenceSource::parse(&source).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?,
                reference: row.get(3)?,
                confidence: row.get(4)?,
                observed_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Links where `entry_id` is the `from` (`outgoing`) or the `to`.
    fn links(&self, entry_id: &str, outgoing: bool) -> Result<Vec<EntryLink>> {
        let column = if outgoing { "from_entry" } else { "to_entry" };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT from_entry, to_entry, kind, created_at FROM ledger_entry_links
             WHERE {column} = ?1 ORDER BY created_at, from_entry, to_entry"
        ))?;
        let rows = stmt.query_map([entry_id], |row| {
            let kind: String = row.get(2)?;
            Ok(EntryLink {
                from_entry: row.get(0)?,
                to_entry: row.get(1)?,
                kind: LinkKind::parse(&kind).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?,
                created_at: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Reads an entry's current state and project, erroring when it is missing.
    fn state_and_project(conn: &Connection, entry_id: &str) -> Result<(EntryState, String)> {
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT state, project FROM ledger_entries WHERE entry_id = ?1",
                [entry_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (state, project) = row.ok_or_else(|| LedgerError::EntryNotFound {
            entry_id: entry_id.to_string(),
        })?;
        Ok((EntryState::parse(&state)?, project))
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Marks an entry resolved, recording how that was established.
    pub fn close(&mut self, entry_id: &str, via: ClosedVia, now: &str) -> Result<LedgerEntry> {
        self.set_state(
            entry_id,
            EntryState::Closed,
            Some(via),
            None,
            None,
            None,
            now,
        )
    }

    /// Marks an entry as deliberately not happening.
    pub fn waive(&mut self, entry_id: &str, now: &str) -> Result<LedgerEntry> {
        self.set_state(entry_id, EntryState::Waived, None, None, None, None, now)
    }

    /// Removes an entry from the working set: it never should have been in it.
    ///
    /// The distinction from [`Ledger::waive`] is the whole point and is worth
    /// keeping straight: waiving says *this was mine and has stopped being
    /// relevant*, which is a judgement about the commitment. Untracking says
    /// *this was never my business*, which is a judgement about the ledger.
    /// Conflating them would lose the only thing retro-application needs to
    /// know when a meeting's tracking flips back.
    ///
    /// The entry's item refs are deliberately left active — see
    /// [`EntryState::Untracked`].
    pub fn untrack(&mut self, entry_id: &str, via: UntrackedVia, now: &str) -> Result<LedgerEntry> {
        self.set_state(
            entry_id,
            EntryState::Untracked,
            None,
            None,
            None,
            Some(via),
            now,
        )
    }

    /// Records that a person has acted on this entry.
    ///
    /// **Deliberately does not bump `updated_at`.** The operation that preceded
    /// this call already did, and a second bump would reorder the settled shelf
    /// for what is really an annotation on the same act. Nor does it mark the
    /// project dirty on its own: every caller is mid-mutation and has already
    /// done so.
    pub fn mark_touched(&mut self, entry_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE ledger_entries SET touched = 1 WHERE entry_id = ?1",
            [entry_id],
        )?;
        Ok(())
    }

    /// Hides an entry until `until` (a local `YYYY-MM-DD` calendar day, like a
    /// due date). Re-snoozing a snoozed entry just moves the date.
    pub fn snooze(&mut self, entry_id: &str, until: &str, now: &str) -> Result<LedgerEntry> {
        if !is_calendar_day(until) {
            return Err(invalid_field(
                "snoozed_until",
                format!("expected a YYYY-MM-DD calendar day, got {until:?}"),
            ));
        }
        self.set_state(
            entry_id,
            EntryState::Snoozed,
            None,
            Some(until),
            None,
            None,
            now,
        )
    }

    /// Parks an entry for a human, with `reason` as the question being asked.
    ///
    /// The machine half of the confidence split: a provider that found
    /// something but was not sure enough to act puts the entry here rather
    /// than guessing. Re-parking an entry already in review replaces the
    /// reason, so the newest question is the one shown.
    pub fn send_to_review(
        &mut self,
        entry_id: &str,
        reason: &str,
        now: &str,
    ) -> Result<LedgerEntry> {
        self.set_state(
            entry_id,
            EntryState::NeedsReview,
            None,
            None,
            Some(reason),
            None,
            now,
        )
    }

    /// Records that a note mentioned this commitment without carrying its line.
    ///
    /// The bare re-mention: somebody brought the commitment up, but no
    /// checkbox in the new note is that commitment, so there is no ref to
    /// link. `last_mention` only ever moves forward, so distilling an old
    /// backlog cannot make a stale entry look fresh, and an entry parked for
    /// review because its line vanished comes back to life on being spoken
    /// about again.
    ///
    /// Deliberately does not move the entry's project the way a mention with a
    /// ref does: with no line in the new note there is nothing anchoring the
    /// commitment there.
    pub fn record_mention(
        &mut self,
        entry_id: &str,
        note_date_utc: &str,
        now: &str,
    ) -> Result<LedgerEntry> {
        let (state, project) = Self::state_and_project(&self.conn, entry_id)?;
        self.conn.execute(
            "UPDATE ledger_entries
             SET last_mention = max(last_mention, ?2), updated_at = ?3
             WHERE entry_id = ?1",
            params![entry_id, note_date_utc, now],
        )?;
        if state == EntryState::NeedsReview {
            self.conn.execute(
                "UPDATE ledger_entries
                 SET state = 'open', review_reason = NULL, updated_at = ?2
                 WHERE entry_id = ?1",
                params![entry_id, now],
            )?;
        }
        self.mark_dirty(&project);
        let detail = self
            .get_entry(entry_id)?
            .ok_or_else(|| LedgerError::EntryNotFound {
                entry_id: entry_id.to_string(),
            })?;
        Ok(detail.entry)
    }

    /// Returns an entry to [`EntryState::Open`], clearing whatever the previous
    /// state carried. Removes the outgoing link that put it in
    /// [`EntryState::Superseded`], so the state and the graph stay consistent.
    pub fn reopen(&mut self, entry_id: &str, now: &str) -> Result<LedgerEntry> {
        let (state, project) = Self::state_and_project(&self.conn, entry_id)?;
        if state == EntryState::Superseded {
            self.conn.execute(
                "DELETE FROM ledger_entry_links WHERE from_entry = ?1",
                [entry_id],
            )?;
        }
        let entry = self.set_state(entry_id, EntryState::Open, None, None, None, None, now)?;
        self.mark_dirty(&project);
        Ok(entry)
    }

    /// The one write path for `state` and its dependent columns, so the
    /// transition table and the `CHECK` constraints can never be bypassed.
    ///
    /// Every dependent column is written on every call, including the ones the
    /// caller left `None`. That is what makes each exit clean up after the last:
    /// reopening a snoozed entry clears its date, and reopening an untracked one
    /// clears its provenance, without either path having to remember to.
    #[allow(clippy::too_many_arguments)]
    fn set_state(
        &mut self,
        entry_id: &str,
        to: EntryState,
        closed_via: Option<ClosedVia>,
        snoozed_until: Option<&str>,
        review_reason: Option<&str>,
        untracked_via: Option<UntrackedVia>,
        now: &str,
    ) -> Result<LedgerEntry> {
        let (from, project) = Self::state_and_project(&self.conn, entry_id)?;
        if !transition_allowed(from, to) {
            return Err(LedgerError::IllegalTransition {
                entry_id: entry_id.to_string(),
                from,
                to,
            });
        }
        self.conn.execute(
            "UPDATE ledger_entries
             SET state = ?2, closed_via = ?3, snoozed_until = ?4, review_reason = ?5,
                 untracked_via = ?6, updated_at = ?7
             WHERE entry_id = ?1",
            params![
                entry_id,
                to.as_str(),
                closed_via.map(ClosedVia::as_str),
                snoozed_until,
                review_reason,
                untracked_via.map(UntrackedVia::as_str),
                now,
            ],
        )?;
        self.mark_dirty(&project);
        let detail = self
            .get_entry(entry_id)?
            .ok_or_else(|| LedgerError::EntryNotFound {
                entry_id: entry_id.to_string(),
            })?;
        Ok(detail.entry)
    }

    // -----------------------------------------------------------------------
    // Evidence
    // -----------------------------------------------------------------------

    /// Records one claim about an entry and stamps the evidence check.
    ///
    /// Recording evidence never changes the entry's state on its own: deciding
    /// what a set of claims *means* belongs to the provider that gathered them,
    /// which calls [`Ledger::close`] separately when it is confident.
    #[allow(clippy::too_many_arguments)]
    pub fn add_evidence(
        &mut self,
        entry_id: &str,
        source: EvidenceSource,
        reference: Option<&str>,
        confidence: f64,
        observed_at: &str,
        now: &str,
    ) -> Result<Evidence> {
        if !(0.0..=1.0).contains(&confidence) || confidence.is_nan() {
            return Err(invalid_field(
                "confidence",
                format!("expected 0.0..=1.0, got {confidence}"),
            ));
        }
        let (_, project) = Self::state_and_project(&self.conn, entry_id)?;
        let evidence_id = mint_id("ev_")?;
        self.conn.execute(
            "INSERT INTO ledger_evidence
                 (evidence_id, entry_id, source, reference, confidence, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                evidence_id,
                entry_id,
                source.as_str(),
                reference,
                confidence,
                observed_at
            ],
        )?;
        self.conn.execute(
            "UPDATE ledger_entries SET last_evidence_check = ?2, updated_at = ?2
             WHERE entry_id = ?1",
            params![entry_id, now],
        )?;
        self.mark_dirty(&project);
        Ok(Evidence {
            evidence_id,
            entry_id: entry_id.to_string(),
            source,
            reference: reference.map(str::to_string),
            confidence,
            observed_at: observed_at.to_string(),
        })
    }

    /// Closes an entry on the strength of one recorded claim, taking the
    /// closure's provenance from that claim's source.
    ///
    /// The confirm half of the confidence split: a provider that was not
    /// confident enough to close parks the entry in
    /// [`EntryState::NeedsReview`] with its claim attached, and a human
    /// confirming it here closes it as `github` or `conversation` rather than
    /// `manual` — the evidence is what resolved it, the human only agreed.
    pub fn close_from_evidence(
        &mut self,
        entry_id: &str,
        evidence_id: &str,
        now: &str,
    ) -> Result<(LedgerEntry, Evidence)> {
        let claim = self
            .evidence_for(entry_id)?
            .into_iter()
            .find(|claim| claim.evidence_id == evidence_id)
            .ok_or_else(|| LedgerError::EvidenceNotFound {
                evidence_id: evidence_id.to_string(),
            })?;
        let entry = self.close(entry_id, claim.source.as_closed_via(), now)?;
        Ok((entry, claim))
    }

    /// Rejects a claim: removes it, and reopens the entry when that claim is
    /// what closed it.
    ///
    /// The dismiss half of the confidence split. Reopening is deliberately
    /// conditional on [`EntryState::Closed`]: dismissing a bad claim on a
    /// snoozed or waived entry answers the claim, and must not also undo a
    /// judgement the human made for unrelated reasons.
    pub fn dismiss_evidence(
        &mut self,
        entry_id: &str,
        evidence_id: &str,
        now: &str,
    ) -> Result<LedgerEntry> {
        // Idempotent in the claim: a retry after a crash finds it already gone
        // and still settles the entry's state below.
        self.remove_evidence(evidence_id, now)?;
        let (state, _) = Self::state_and_project(&self.conn, entry_id)?;
        if state == EntryState::Closed {
            return self.reopen(entry_id, now);
        }
        let detail = self
            .get_entry(entry_id)?
            .ok_or_else(|| LedgerError::EntryNotFound {
                entry_id: entry_id.to_string(),
            })?;
        Ok(detail.entry)
    }

    /// Stamps `last_evidence_check` without recording a claim — a provider that
    /// looked and found nothing still made the entry less stale to look at again.
    pub fn touch_evidence_check(&mut self, entry_id: &str, now: &str) -> Result<()> {
        let (_, project) = Self::state_and_project(&self.conn, entry_id)?;
        self.conn.execute(
            "UPDATE ledger_entries SET last_evidence_check = ?2, updated_at = ?2
             WHERE entry_id = ?1",
            params![entry_id, now],
        )?;
        self.mark_dirty(&project);
        Ok(())
    }

    /// Removes a claim (a provider retracting a false positive). Returns whether
    /// a row was removed.
    pub fn remove_evidence(&mut self, evidence_id: &str, now: &str) -> Result<bool> {
        let entry_id: Option<String> = self
            .conn
            .query_row(
                "SELECT entry_id FROM ledger_evidence WHERE evidence_id = ?1",
                [evidence_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(entry_id) = entry_id else {
            return Ok(false);
        };
        self.conn.execute(
            "DELETE FROM ledger_evidence WHERE evidence_id = ?1",
            [evidence_id],
        )?;
        self.conn.execute(
            "UPDATE ledger_entries SET updated_at = ?2 WHERE entry_id = ?1",
            params![entry_id, now],
        )?;
        let (_, project) = Self::state_and_project(&self.conn, &entry_id)?;
        self.mark_dirty(&project);
        Ok(true)
    }

    // -----------------------------------------------------------------------
    // Links and manual re-linking
    // -----------------------------------------------------------------------

    /// Records that `from` is replaced by (or merged into) `to`, and moves
    /// `from` to [`EntryState::Superseded`].
    ///
    /// This is the manual half of "one commitment, many mentions": when two
    /// entries turn out to be the same promise worded differently, linking them
    /// leaves one live entry and keeps the earlier one as history.
    pub fn link_entries(
        &mut self,
        from: &str,
        to: &str,
        kind: LinkKind,
        now: &str,
    ) -> Result<EntryLink> {
        if from == to {
            return Err(invalid_field(
                "to_entry",
                "an entry cannot supersede itself",
            ));
        }
        let (from_state, from_project) = Self::state_and_project(&self.conn, from)?;
        let (_, to_project) = Self::state_and_project(&self.conn, to)?;
        // Checked against the whole transition table *before* the edge is
        // written, not just for `Closed`: the insert and the state change are
        // two statements on a bare connection, so a transition rejected after
        // the insert would leave a live edge on an entry that never became
        // superseded — the exact drift this state is meant to make impossible.
        if !transition_allowed(from_state, EntryState::Superseded) {
            return Err(LedgerError::IllegalTransition {
                entry_id: from.to_string(),
                from: from_state,
                to: EntryState::Superseded,
            });
        }
        self.conn.execute(
            "INSERT OR REPLACE INTO ledger_entry_links (from_entry, to_entry, kind, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![from, to, kind.as_str(), now],
        )?;
        self.set_state(from, EntryState::Superseded, None, None, None, None, now)?;
        self.mark_dirty(&from_project);
        self.mark_dirty(&to_project);
        Ok(EntryLink {
            from_entry: from.to_string(),
            to_entry: to.to_string(),
            kind,
            created_at: now.to_string(),
        })
    }

    /// Removes a link. The `from` entry stays superseded — undoing the judgement
    /// is [`Ledger::reopen`]'s job, which removes the link itself. Returns
    /// whether a row was removed.
    pub fn unlink_entries(&mut self, from: &str, to: &str, now: &str) -> Result<bool> {
        let removed = self.conn.execute(
            "DELETE FROM ledger_entry_links WHERE from_entry = ?1 AND to_entry = ?2",
            params![from, to],
        )?;
        if removed == 0 {
            return Ok(false);
        }
        let (_, project) = Self::state_and_project(&self.conn, from)?;
        self.conn.execute(
            "UPDATE ledger_entries SET updated_at = ?2 WHERE entry_id = ?1",
            params![from, now],
        )?;
        self.mark_dirty(&project);
        Ok(true)
    }

    /// Attaches a live extracted line to `entry_id` by hand, retiring whichever
    /// entry currently holds it.
    ///
    /// The escape hatch for everything automatic matching declines to guess: two
    /// lines worded differently, an ambiguous edit, a commitment that moved
    /// between notes. An entry that landed in review because its line vanished
    /// returns to [`EntryState::Open`].
    pub fn relink_item(
        &mut self,
        entry_id: &str,
        note_id: &str,
        item_id: &str,
        now: &str,
    ) -> Result<()> {
        let (state, project) = Self::state_and_project(&self.conn, entry_id)?;
        // Retire any other entry's claim on this line first: the partial unique
        // index would otherwise reject the insert.
        self.conn.execute(
            "UPDATE ledger_item_refs SET active = 0, retired_at = ?3
             WHERE note_id = ?1 AND item_id = ?2 AND active = 1 AND entry_id <> ?4",
            params![note_id, item_id, now, entry_id],
        )?;
        self.conn.execute(
            "INSERT INTO ledger_item_refs (entry_id, item_id, note_id, active, linked_at)
             VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT (entry_id, note_id, item_id)
             DO UPDATE SET active = 1, retired_at = NULL, linked_at = ?4",
            params![entry_id, item_id, note_id, now],
        )?;
        self.conn.execute(
            "UPDATE ledger_entries SET updated_at = ?2 WHERE entry_id = ?1",
            params![entry_id, now],
        )?;
        if state == EntryState::NeedsReview {
            self.set_state(entry_id, EntryState::Open, None, None, None, None, now)?;
        }
        self.mark_dirty(&project);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Internals shared with `sync` and `snapshot`
    // -----------------------------------------------------------------------

    /// Inserts an entry exactly as given. Used by [`super::sync`] (which mints
    /// the id) and [`super::snapshot`] (which restores one).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_entry(conn: &Connection, entry: &LedgerEntry) -> Result<()> {
        conn.execute(
            "INSERT INTO ledger_entries
                 (entry_id, state, direction, owner, description, owner_norm, description_norm,
                  project, created_at, updated_at, last_mention, last_evidence_check,
                  snoozed_until, closed_via, review_reason, enrolled_via, untracked_via, touched)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                     ?18)",
            params![
                entry.entry_id,
                entry.state.as_str(),
                entry.direction.as_str(),
                entry.owner,
                entry.description,
                normalize(&entry.owner),
                normalize(&entry.description),
                entry.project,
                entry.created_at,
                entry.updated_at,
                entry.last_mention,
                entry.last_evidence_check,
                entry.snoozed_until,
                entry.closed_via.map(ClosedVia::as_str),
                entry.review_reason,
                entry.enrolled_via.as_str(),
                entry.untracked_via.map(UntrackedVia::as_str),
                i64::from(entry.touched),
            ],
        )?;
        Ok(())
    }

    /// Mutably borrows the connection, for a transaction.
    pub(crate) fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// Marks several projects dirty at once.
    pub(crate) fn mark_all_dirty(&mut self, projects: BTreeSet<String>) {
        for project in projects {
            self.mark_dirty(&project);
        }
    }
}

/// Whether `value` is a `YYYY-MM-DD` calendar day.
fn is_calendar_day(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::sync::NoteSync;
    use crate::meeting::ActionItemFact;

    const NOW: &str = "2026-08-17T12:00:00Z";
    const DAY: &str = "2026-08-17T00:00:00Z";

    fn fact(id: &str, owner: &str, description: &str) -> ActionItemFact {
        ActionItemFact {
            id: id.to_string(),
            description: description.to_string(),
            owner: owner.to_string(),
            due_date: None,
            done: false,
            extracted_date: Some("2026-08-17".to_string()),
        }
    }

    /// A ledger holding one open entry for `a_111111` in `n_a1b2c3`.
    fn seeded() -> (Ledger, String) {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the revised deck")];
        let outcome = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap();
        let entry_id = outcome.created[0].clone();
        (ledger, entry_id)
    }

    #[test]
    fn list_details_hydrates_and_keeps_list_entries_order() {
        let (mut ledger, entry_id) = seeded();
        ledger
            .add_evidence(
                &entry_id,
                EvidenceSource::Github,
                Some("https://example.com/pull/42"),
                0.9,
                NOW,
                NOW,
            )
            .unwrap();

        let details = ledger.list_details(&EntryFilter::default()).unwrap();
        let entries = ledger.list_entries(&EntryFilter::default()).unwrap();

        assert_eq!(
            details.iter().map(|d| &d.entry).collect::<Vec<_>>(),
            entries.iter().collect::<Vec<_>>(),
            "hydration must not reorder or alter the entries"
        );
        let detail = &details[0];
        assert_eq!(detail.item_refs.len(), 1);
        assert_eq!(detail.evidence.len(), 1);
        assert_eq!(detail.evidence[0].confidence, 0.9);
    }

    #[test]
    fn close_from_evidence_takes_its_provenance_from_the_claim() {
        let (mut ledger, entry_id) = seeded();
        let claim = ledger
            .add_evidence(
                &entry_id,
                EvidenceSource::Github,
                Some("https://example.com/pull/42"),
                0.94,
                NOW,
                NOW,
            )
            .unwrap();

        let (entry, used) = ledger
            .close_from_evidence(&entry_id, &claim.evidence_id, NOW)
            .unwrap();

        // `github`, not `manual`: the evidence resolved it, the human agreed.
        assert_eq!(entry.state, EntryState::Closed);
        assert_eq!(entry.closed_via, Some(ClosedVia::Github));
        assert_eq!(used.evidence_id, claim.evidence_id);
    }

    #[test]
    fn close_from_evidence_rejects_a_claim_that_is_not_the_entrys() {
        let (mut ledger, entry_id) = seeded();
        let err = ledger
            .close_from_evidence(&entry_id, "ev_nosuchclaim", NOW)
            .unwrap_err();
        assert!(matches!(err, LedgerError::EvidenceNotFound { .. }));
    }

    #[test]
    fn dismiss_evidence_reopens_a_closed_entry_but_leaves_a_snooze_alone() {
        let (mut ledger, entry_id) = seeded();
        let claim = ledger
            .add_evidence(&entry_id, EvidenceSource::Github, None, 0.8, NOW, NOW)
            .unwrap();
        ledger
            .close_from_evidence(&entry_id, &claim.evidence_id, NOW)
            .unwrap();

        let entry = ledger
            .dismiss_evidence(&entry_id, &claim.evidence_id, NOW)
            .unwrap();

        assert_eq!(
            entry.state,
            EntryState::Open,
            "a rejected claim undoes its own closure"
        );
        assert_eq!(entry.closed_via, None);
        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert!(detail.evidence.is_empty(), "the claim itself is gone");

        // A snoozed entry keeps its snooze: the claim is answered, the human's
        // unrelated judgement is not undone.
        let (mut ledger, entry_id) = seeded();
        let claim = ledger
            .add_evidence(&entry_id, EvidenceSource::Github, None, 0.5, NOW, NOW)
            .unwrap();
        ledger.snooze(&entry_id, "2026-09-01", NOW).unwrap();
        let entry = ledger
            .dismiss_evidence(&entry_id, &claim.evidence_id, NOW)
            .unwrap();
        assert_eq!(entry.state, EntryState::Snoozed);
        assert_eq!(entry.snoozed_until.as_deref(), Some("2026-09-01"));
    }

    #[test]
    fn dismiss_evidence_is_idempotent_in_the_claim() {
        let (mut ledger, entry_id) = seeded();
        let claim = ledger
            .add_evidence(&entry_id, EvidenceSource::Github, None, 0.8, NOW, NOW)
            .unwrap();
        ledger
            .dismiss_evidence(&entry_id, &claim.evidence_id, NOW)
            .unwrap();
        // A retry after a crash finds the claim gone and still answers.
        let entry = ledger
            .dismiss_evidence(&entry_id, &claim.evidence_id, NOW)
            .unwrap();
        assert_eq!(entry.state, EntryState::Open);
    }

    #[test]
    fn normalize_collapses_case_and_whitespace() {
        assert_eq!(normalize("  Send   the\tDECK \n"), "send the deck");
        assert_eq!(normalize("Priya"), "priya");
        // NFC: a composed and a decomposed spelling compare equal.
        assert_eq!(normalize("Zoe\u{0301}"), normalize("Zo\u{00e9}"));
    }

    #[test]
    fn close_records_provenance_and_waive_does_not() {
        let (mut ledger, entry_id) = seeded();
        let entry = ledger.close(&entry_id, ClosedVia::Github, NOW).unwrap();
        assert_eq!(entry.state, EntryState::Closed);
        assert_eq!(entry.closed_via, Some(ClosedVia::Github));

        let (mut ledger, entry_id) = seeded();
        let entry = ledger.waive(&entry_id, NOW).unwrap();
        assert_eq!(entry.state, EntryState::Waived);
        assert_eq!(entry.closed_via, None);
    }

    #[test]
    fn snooze_requires_a_calendar_day_and_reopen_clears_it() {
        let (mut ledger, entry_id) = seeded();
        assert!(ledger
            .snooze(&entry_id, "2026-08-20T00:00:00Z", NOW)
            .is_err());
        assert!(ledger.snooze(&entry_id, "not-a-day", NOW).is_err());

        let entry = ledger.snooze(&entry_id, "2026-08-20", NOW).unwrap();
        assert_eq!(entry.state, EntryState::Snoozed);
        assert_eq!(entry.snoozed_until.as_deref(), Some("2026-08-20"));

        // Re-snoozing just moves the date.
        let entry = ledger.snooze(&entry_id, "2026-08-25", NOW).unwrap();
        assert_eq!(entry.snoozed_until.as_deref(), Some("2026-08-25"));

        let entry = ledger.reopen(&entry_id, NOW).unwrap();
        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.snoozed_until, None);
    }

    #[test]
    fn untracking_records_its_provenance_and_reopening_clears_it() {
        let (mut ledger, entry_id) = seeded();

        let entry = ledger
            .untrack(&entry_id, UntrackedVia::Override, NOW)
            .unwrap();
        assert_eq!(entry.state, EntryState::Untracked);
        assert_eq!(entry.untracked_via, Some(UntrackedVia::Override));
        // Untrack is about the ledger, not about the commitment, so it makes no
        // claim that anything was resolved.
        assert_eq!(entry.closed_via, None);

        let entry = ledger.reopen(&entry_id, NOW).unwrap();
        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.untracked_via, None, "the single writer cleans up");
    }

    #[test]
    fn untracking_leaves_the_item_refs_active() {
        let (mut ledger, entry_id) = seeded();
        ledger
            .untrack(&entry_id, UntrackedVia::Manual, NOW)
            .unwrap();

        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert!(
            detail.item_refs.iter().any(|item| item.active),
            "an untracked entry still owns its line, which is what keeps a \
             re-sync from minting a duplicate or parking it in review"
        );
    }

    #[test]
    fn marking_touched_records_the_person_without_reordering_the_shelf() {
        let (mut ledger, entry_id) = seeded();
        let before = ledger.get_entry(&entry_id).unwrap().unwrap().entry;
        assert!(!before.touched);

        ledger.mark_touched(&entry_id).unwrap();

        let after = ledger.get_entry(&entry_id).unwrap().unwrap().entry;
        assert!(after.touched);
        assert_eq!(
            after.updated_at, before.updated_at,
            "the op that preceded this already stamped it"
        );
    }

    #[test]
    fn the_transition_table_is_enforced() {
        use EntryState::*;
        // Every pair the table forbids, checked through the public API.
        let cases: &[(EntryState, EntryState)] = &[
            (Closed, NeedsReview),
            (Closed, Waived),
            (Closed, Snoozed),
            (Waived, Closed),
            (Waived, NeedsReview),
            (Waived, Snoozed),
            (NeedsReview, Snoozed),
            // Untracking something already settled would overwrite a real
            // judgement with a weaker one.
            (Closed, Untracked),
            (Waived, Untracked),
            (Superseded, Untracked),
            // And an untracked entry leaves only by being tracked again.
            (Untracked, Closed),
            (Untracked, Waived),
            (Untracked, Snoozed),
            (Untracked, NeedsReview),
        ];
        for &(from, to) in cases {
            assert!(
                !transition_allowed(from, to),
                "{from} -> {to} should be illegal"
            );
        }
        for &(from, to) in &[
            (Open, Closed),
            (Open, Waived),
            (Open, Snoozed),
            (Open, NeedsReview),
            (Snoozed, Open),
            (Snoozed, NeedsReview),
            (NeedsReview, Open),
            (NeedsReview, Closed),
            (Closed, Open),
            (Waived, Open),
            (Superseded, Open),
            (Open, Untracked),
            (NeedsReview, Untracked),
            (Snoozed, Untracked),
            (Untracked, Open),
        ] {
            assert!(
                transition_allowed(from, to),
                "{from} -> {to} should be legal"
            );
        }

        // And the error actually surfaces.
        let (mut ledger, entry_id) = seeded();
        ledger.waive(&entry_id, NOW).unwrap();
        let err = ledger.close(&entry_id, ClosedVia::Manual, NOW).unwrap_err();
        assert!(
            matches!(err, LedgerError::IllegalTransition { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn evidence_is_recorded_without_changing_state() {
        let (mut ledger, entry_id) = seeded();
        let evidence = ledger
            .add_evidence(
                &entry_id,
                EvidenceSource::Github,
                Some("https://example.com/pull/42"),
                0.8,
                "2026-08-16T02:00:00Z",
                NOW,
            )
            .unwrap();
        assert!(evidence.evidence_id.starts_with("ev_"));

        let detail = ledger.get_entry(&entry_id).unwrap().unwrap();
        assert_eq!(detail.evidence.len(), 1);
        assert_eq!(
            detail.entry.state,
            EntryState::Open,
            "evidence alone never closes an entry"
        );
        assert_eq!(detail.entry.last_evidence_check.as_deref(), Some(NOW));

        assert!(ledger.remove_evidence(&evidence.evidence_id, NOW).unwrap());
        assert!(!ledger.remove_evidence("ev_missing00000", NOW).unwrap());
    }

    #[test]
    fn evidence_confidence_is_bounded() {
        let (mut ledger, entry_id) = seeded();
        for bad in [-0.1, 1.1, f64::NAN] {
            let err = ledger
                .add_evidence(&entry_id, EvidenceSource::Manual, None, bad, NOW, NOW)
                .unwrap_err();
            assert!(matches!(err, LedgerError::InvalidField { .. }), "{bad}");
        }
    }

    #[test]
    fn linking_supersedes_the_source_and_reopen_removes_the_link() {
        let (mut ledger, first) = seeded();
        let items = vec![fact("a_222222", "Priya", "send the final deck")];
        let second = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf",
                note_date_utc: "2026-08-18T00:00:00Z",
                items: &items,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap()
            .created[0]
            .clone();

        ledger
            .link_entries(&first, &second, LinkKind::Supersedes, NOW)
            .unwrap();
        let detail = ledger.get_entry(&first).unwrap().unwrap();
        assert_eq!(detail.entry.state, EntryState::Superseded);
        assert_eq!(detail.links_out.len(), 1);
        assert_eq!(
            ledger.get_entry(&second).unwrap().unwrap().links_in.len(),
            1
        );

        // Reopening undoes the judgement and the edge together.
        ledger.reopen(&first, NOW).unwrap();
        let detail = ledger.get_entry(&first).unwrap().unwrap();
        assert_eq!(detail.entry.state, EntryState::Open);
        assert!(detail.links_out.is_empty());
    }

    #[test]
    fn linking_rejects_self_links_and_closed_sources() {
        let (mut ledger, entry_id) = seeded();
        assert!(ledger
            .link_entries(&entry_id, &entry_id, LinkKind::Supersedes, NOW)
            .is_err());

        let items = vec![fact("a_222222", "Priya", "send the final deck")];
        let other = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap()
            .created[0]
            .clone();
        ledger.close(&entry_id, ClosedVia::Manual, NOW).unwrap();
        let err = ledger
            .link_entries(&entry_id, &other, LinkKind::Supersedes, NOW)
            .unwrap_err();
        assert!(matches!(err, LedgerError::IllegalTransition { .. }));
    }

    #[test]
    fn relink_item_steals_a_live_ref_and_resolves_review() {
        let (mut ledger, first) = seeded();
        let items = vec![fact("a_222222", "Sam", "book the venue")];
        let second = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap()
            .created[0]
            .clone();

        // Delete `second`'s own note, which strands it in review, then hand it
        // the line `first` owns.
        ledger.note_removed("n_d4e5f6", NOW).unwrap();
        assert_eq!(
            ledger.get_entry(&second).unwrap().unwrap().entry.state,
            EntryState::NeedsReview
        );
        ledger
            .relink_item(&second, "n_a1b2c3", "a_111111", NOW)
            .unwrap();

        let detail = ledger.get_entry(&second).unwrap().unwrap();
        assert_eq!(detail.entry.state, EntryState::Open, "review resolved");
        assert!(detail
            .item_refs
            .iter()
            .any(|r| r.item_id == "a_111111" && r.active));

        // The previous owner keeps the ref, retired.
        let previous = ledger.get_entry(&first).unwrap().unwrap();
        let stolen = previous
            .item_refs
            .iter()
            .find(|r| r.item_id == "a_111111")
            .unwrap();
        assert!(!stolen.active);
        assert_eq!(stolen.retired_at.as_deref(), Some(NOW));
    }

    #[test]
    fn list_entries_filters_and_orders() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let older = vec![fact("a_111111", "Priya", "send the deck")];
        let newer = vec![fact("a_222222", "You", "book the venue")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood Golf",
                note_date_utc: "2026-08-01T00:00:00Z",
                items: &older,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap();
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf/Range",
                note_date_utc: "2026-08-10T00:00:00Z",
                items: &newer,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap();

        // Newest mention first.
        let all = ledger.list_entries(&EntryFilter::default()).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].description, "book the venue");

        // Exact project excludes the child; descendants include it.
        let exact = ledger
            .list_entries(&EntryFilter {
                project: Some("Briarwood Golf".to_string()),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(exact.len(), 1);
        let wide = ledger
            .list_entries(&EntryFilter {
                project: Some("Briarwood Golf".to_string()),
                include_descendants: true,
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(wide.len(), 2);

        // Direction, note, staleness, and the never-checked queue.
        let mine = ledger
            .list_entries(&EntryFilter {
                direction: Some(Direction::Mine),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].owner, "You");

        let in_note = ledger
            .list_entries(&EntryFilter {
                note_id: Some("n_a1b2c3".to_string()),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(in_note.len(), 1);

        let stale = ledger
            .list_entries(&EntryFilter {
                stale_before: Some("2026-08-05T00:00:00Z".to_string()),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].description, "send the deck");

        let unchecked = ledger
            .list_entries(&EntryFilter {
                unchecked_since: Some(NOW.to_string()),
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(unchecked.len(), 2, "nothing has been checked yet");

        // An explicitly empty state set is a question with no answers.
        let none = ledger
            .list_entries(&EntryFilter {
                states: Some(vec![]),
                ..EntryFilter::default()
            })
            .unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn a_project_slug_with_sql_wildcards_is_not_a_wildcard() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the deck")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Ops_West",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap();
        let matched = ledger
            .list_entries(&EntryFilter {
                // `_` is LIKE's single-character wildcard; a prefix compare must
                // not treat it as one.
                project: Some("Ops%West".to_string()),
                include_descendants: true,
                ..EntryFilter::default()
            })
            .unwrap();
        assert!(matched.is_empty());
    }

    #[test]
    fn descendants_match_a_non_ascii_project_slug() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        let items = vec![fact("a_111111", "Priya", "send the deck")];
        ledger
            .sync_note_items(&NoteSync {
                note_id: "n_a1b2c3",
                project: "Café/Q3",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap();
        let wide = ledger
            .list_entries(&EntryFilter {
                project: Some("Café".to_string()),
                include_descendants: true,
                ..EntryFilter::default()
            })
            .unwrap();
        assert_eq!(wide.len(), 1, "a child of a non-ASCII slug is a descendant");
    }

    #[test]
    fn a_rejected_link_leaves_no_edge_behind() {
        let (mut ledger, first) = seeded();
        let items = vec![fact("a_222222", "Priya", "send the final deck")];
        let second = ledger
            .sync_note_items(&NoteSync {
                note_id: "n_d4e5f6",
                project: "Briarwood Golf",
                note_date_utc: DAY,
                items: &items,
                link_hints: &[],
                note_override: None,
                now: NOW,
            })
            .unwrap()
            .created[0]
            .clone();

        ledger.waive(&first, NOW).unwrap();
        let err = ledger
            .link_entries(&first, &second, LinkKind::Supersedes, NOW)
            .unwrap_err();
        assert!(
            matches!(err, LedgerError::IllegalTransition { .. }),
            "{err:?}"
        );

        // The refusal must be total: a stored edge whose `from` is not
        // superseded is exactly the drift the state machine exists to prevent.
        let detail = ledger.get_entry(&first).unwrap().unwrap();
        assert_eq!(detail.entry.state, EntryState::Waived);
        assert!(detail.links_out.is_empty(), "{:?}", detail.links_out);
        assert!(ledger
            .get_entry(&second)
            .unwrap()
            .unwrap()
            .links_in
            .is_empty());
    }

    #[test]
    fn missing_entries_are_reported_not_panicked() {
        let mut ledger = Ledger::open_in_memory().unwrap();
        assert!(ledger.get_entry("le_missing00000").unwrap().is_none());
        let err = ledger
            .close("le_missing00000", ClosedVia::Manual, NOW)
            .unwrap_err();
        assert!(matches!(err, LedgerError::EntryNotFound { .. }));
    }

    #[test]
    fn send_to_review_parks_a_live_entry_and_refuses_a_settled_one() {
        let (mut ledger, entry_id) = seeded();

        let entry = ledger
            .send_to_review(&entry_id, "a conversation reported this done", NOW)
            .unwrap();
        assert_eq!(entry.state, EntryState::NeedsReview);
        assert_eq!(
            entry.review_reason.as_deref(),
            Some("a conversation reported this done")
        );

        // Re-parking replaces the question rather than stacking another.
        let entry = ledger.send_to_review(&entry_id, "and again", NOW).unwrap();
        assert_eq!(entry.review_reason.as_deref(), Some("and again"));

        // A snoozed entry can still be parked; a closed one cannot, because
        // the ledger does not get to reopen what a human settled.
        ledger.close(&entry_id, ClosedVia::Manual, NOW).unwrap();
        assert!(matches!(
            ledger.send_to_review(&entry_id, "too late", NOW),
            Err(LedgerError::IllegalTransition { .. })
        ));
    }

    #[test]
    fn record_mention_moves_the_clock_forward_only() {
        let (mut ledger, entry_id) = seeded();
        let before = ledger.get_entry(&entry_id).unwrap().unwrap().entry;

        let entry = ledger
            .record_mention(&entry_id, "2026-09-01T00:00:00Z", NOW)
            .unwrap();
        assert_eq!(entry.last_mention, "2026-09-01T00:00:00Z");

        // An older note never drags it back: re-indexing a backlog must not
        // rewrite when a commitment was last heard about.
        let entry = ledger
            .record_mention(&entry_id, "2026-01-01T00:00:00Z", NOW)
            .unwrap();
        assert_eq!(entry.last_mention, "2026-09-01T00:00:00Z");
        assert_ne!(entry.last_mention, before.last_mention);
    }

    #[test]
    fn record_mention_revives_an_entry_parked_for_review() {
        let (mut ledger, entry_id) = seeded();
        ledger
            .send_to_review(&entry_id, "its line vanished", NOW)
            .unwrap();

        // Being spoken about again answers the question the review was asking.
        let entry = ledger.record_mention(&entry_id, DAY, NOW).unwrap();
        assert_eq!(entry.state, EntryState::Open);
        assert_eq!(entry.review_reason, None);
    }

    #[test]
    fn record_mention_leaves_a_snooze_alone() {
        let (mut ledger, entry_id) = seeded();
        ledger.snooze(&entry_id, "2026-12-01", NOW).unwrap();

        // Mentioning something you deliberately shelved is not a reason to
        // unshelve it: the snooze is the user's decision, not the ledger's.
        let entry = ledger.record_mention(&entry_id, DAY, NOW).unwrap();
        assert_eq!(entry.state, EntryState::Snoozed);
        assert_eq!(entry.snoozed_until.as_deref(), Some("2026-12-01"));
    }
}
