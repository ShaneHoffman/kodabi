//! The daily digest: what *changed* in the commitment ledger since the last
//! time it was computed.
//!
//! The ledger only exists if someone opens the Commitments view. This module is
//! the other direction — once a day, a short, ranked list of the transitions
//! worth knowing about, rendered onto two surfaces from one computation: a card
//! in the app and a note in the vault.
//!
//! # News, not a second copy of the view
//!
//! An item appears here when it *crosses* a boundary, never while it merely
//! sits past one. That is the whole difference between a digest and a filtered
//! list: a commitment that went stale on Tuesday is news on Tuesday and noise
//! every day after. Newly enrolled commitments are deliberately absent — the
//! triage strip owns those, and the two surfaces would otherwise report the
//! same event twice.
//!
//! # Why the transitions are derived twice
//!
//! Nothing in the ledger is written when a tier turns or a due date passes
//! ([`super::view::AgingTier`] is derived, never stored, for the same reason
//! [`super::EntryState::Snoozed`] has no expiry writer). So there is no
//! transition log to read, and the digest reconstructs one: every rule below
//! evaluates the *same* pure derivation at two dates, `baseline` (the day the
//! last digest ran) and `today`, and reports the entries whose answer changed.
//! That makes the whole computation a function of the ledger row plus two
//! dates, with no state of its own beyond the marker naming the baseline.
//!
//! # The trap this module is shaped around
//!
//! The digest note is a `type: note`, and the plain-checkbox grammar
//! ([`crate::meeting`]) reads **every** `- [ ]` / `- [x]` line in such a body as
//! one of the user's own action items, wherever in the body it sits — no
//! `## Action items` heading required, and `tracking: context-only` does not
//! gate it, because the items it mints are self-owned. A digest that listed
//! commitments as checkboxes would therefore be re-enrolled as a fresh set of
//! commitments on the next reconcile, every day, compounding.
//!
//! So [`render_note_body`] emits plain `- ` bullets and never a checkbox, and
//! [`bullet`] defends the one composition that could still produce one by
//! accident: a commitment whose *description* happens to begin `[ ] `.

use std::collections::HashMap;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::view::{snooze_lapsed, AgingConfig, AgingTier, NoteContext};
use super::{Direction, EntryDetail, EntryState};
use crate::index::{ActionItemRow, ActionItemStatus};
use crate::note::{Note, NoteId, NoteType, Routing, Source, SourceKeyword};

/// How many transitions a digest carries.
///
/// Small on purpose. The digest competes with the thing the user actually
/// opened the app to do, and a list long enough to need triage is a second
/// inbox rather than a glance. What does not fit is counted, not hidden
/// ([`Digest::more`]), and the Commitments view is where the full picture
/// lives.
pub const DIGEST_CAP: usize = 5;

/// Days a commitment someone *else* owes can go unmentioned before the digest
/// raises it.
///
/// Shorter than the aging threshold on purpose: the two answer different
/// questions. Aging asks whether *you* have forgotten something; this asks
/// whether it is time to ask someone about it, and the useful moment for that
/// is before the trail goes cold, while the last conversation is still
/// recallable.
pub const DEFAULT_QUIET_AFTER_DAYS: u32 = 10;

/// The vault folder digest notes are written to.
///
/// A real project folder rather than a reserved root, because the point of
/// writing the note at all is that it is searchable and visible over MCP, and
/// a reserved root is walked by neither the reconcile nor the watcher. The
/// cost is that it reads as a project in the sidebar, which is accepted: a
/// folder of daily digests is a fair description of what it is.
pub const DIGESTS_PROJECT: &str = "Digests";

/// Why a commitment is in today's digest.
///
/// One kind per entry, even when an entry crosses two boundaries on the same
/// day: the digest reports the newest true thing about a commitment, not every
/// true thing (see [`classify`] for the precedence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestKind {
    /// The due date passed since the last digest.
    NewlyOverdue,
    /// The entry entered [`EntryState::NeedsReview`] since the last digest —
    /// an evidence claim awaiting judgement, or a source line that vanished.
    ParkedInReview,
    /// The aging tier reached [`AgingTier::Stale`] since the last digest.
    WentStale,
    /// A commitment someone else owes passed the quiet threshold since the
    /// last digest.
    TheirsQuiet,
}

impl DigestKind {
    /// The wire spelling, matching [`super::EntryState::as_str`]'s convention.
    pub fn as_str(self) -> &'static str {
        match self {
            DigestKind::NewlyOverdue => "newly_overdue",
            DigestKind::ParkedInReview => "parked_in_review",
            DigestKind::WentStale => "went_stale",
            DigestKind::TheirsQuiet => "theirs_quiet",
        }
    }

    /// Rank among kinds, lowest first. Overdue leads because it is the only
    /// kind with a date the user has already missed; the two silence kinds
    /// trail because they are prompts rather than obligations.
    fn rank(self) -> u8 {
        match self {
            DigestKind::NewlyOverdue => 0,
            DigestKind::ParkedInReview => 1,
            DigestKind::WentStale => 2,
            DigestKind::TheirsQuiet => 3,
        }
    }
}

/// One transition, with the context a surface needs to render it without a
/// second lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestItem {
    pub entry_id: String,
    pub kind: DigestKind,
    /// The live source line's text where one exists, else the entry's cached
    /// description — the same fallback [`super::view::assemble`] makes, and
    /// why the entry caches it.
    pub description: String,
    pub owner: String,
    /// Project of the source note, falling back to the entry's own.
    pub project: String,
    pub note_id: Option<String>,
    pub note_title: Option<String>,
    /// Present on [`DigestKind::NewlyOverdue`].
    pub due_date: Option<String>,
    /// Present on [`DigestKind::TheirsQuiet`] and [`DigestKind::WentStale`]:
    /// the note date the commitment was last mentioned on.
    pub last_mention: Option<String>,
    /// Present on [`DigestKind::TheirsQuiet`]: whole days since that mention.
    pub quiet_days: Option<u32>,
    /// Present on [`DigestKind::ParkedInReview`]: the entry's own sentence
    /// saying why a human is needed.
    pub review_reason: Option<String>,
}

/// A day's digest: the ranked transitions, capped, plus what the cap dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digest {
    /// The local calendar day this digest describes (`YYYY-MM-DD`).
    pub date: String,
    /// The local calendar day it measures *from* — the previous digest's day.
    pub since: String,
    pub items: Vec<DigestItem>,
    /// Transitions that qualified but did not fit [`DIGEST_CAP`].
    pub more: u32,
}

impl Digest {
    /// An empty digest for `today`, which is what a first run and a quiet day
    /// both produce. Rendered by no surface: the card hides and no note is
    /// written.
    pub fn empty(today: NaiveDate, since: NaiveDate) -> Self {
        Digest {
            date: today.to_string(),
            since: since.to_string(),
            items: Vec::new(),
            more: 0,
        }
    }

    /// Whether there is anything to show. A digest with nothing in it is not a
    /// surface with an empty state — it is the absence of a surface.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// Computes the transitions between `baseline_date` and `today`.
///
/// `details` are the ledger's live entries (open, snoozed, needs-review);
/// `notes` are the index's rows for the notes they point at, exactly as
/// [`super::view::assemble`] takes them. `baseline_instant` is the previous
/// digest's RFC 3339 UTC marker, compared lexically against `updated_at` for
/// the one rule that is about a stored state change rather than a derived one.
///
/// Pure and clock-free: both dates are arguments, so every rule is
/// deterministically testable at any point in a commitment's life.
pub fn compute(
    details: &[EntryDetail],
    notes: &HashMap<String, NoteContext>,
    baseline_date: NaiveDate,
    baseline_instant: &str,
    today: NaiveDate,
    aging: AgingConfig,
    quiet_after_days: u32,
) -> Digest {
    // A baseline at or after today would make every "crossed since" test
    // vacuous, and a *later* baseline would make them nonsense. Both mean the
    // caller's marker guard has already decided a digest is not due, so this
    // is belt-and-braces rather than a real path.
    if baseline_date >= today {
        return Digest::empty(today, baseline_date);
    }

    let mut ranked: Vec<(SortKey, DigestItem)> = details
        .iter()
        .filter_map(|detail| {
            let live = live_row(detail, notes);
            let kind = classify(
                detail,
                live,
                baseline_date,
                baseline_instant,
                today,
                aging,
                quiet_after_days,
            )?;
            Some(build(detail, live, kind, notes, today))
        })
        .collect();

    ranked.sort_by(|(left, _), (right, _)| left.cmp(right));

    let more = ranked.len().saturating_sub(DIGEST_CAP);
    let items = ranked
        .into_iter()
        .take(DIGEST_CAP)
        .map(|(_, item)| item)
        .collect();

    Digest {
        date: today.to_string(),
        since: baseline_date.to_string(),
        items,
        more: u32::try_from(more).unwrap_or(u32::MAX),
    }
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// The live source line behind an entry, when it still has one.
///
/// Refs come back active-first, and only a live ref names a line the note still
/// holds — the same rule [`super::view::assemble`] applies, for the same
/// reason.
fn live_row<'a>(
    detail: &EntryDetail,
    notes: &'a HashMap<String, NoteContext>,
) -> Option<&'a ActionItemRow> {
    let active = detail.item_refs.iter().find(|item_ref| item_ref.active)?;
    let note = notes.get(&active.note_id)?;
    note.items.iter().find(|row| row.id == active.item_id)
}

/// Whether an entry is in the working set on `today`: open, or a snooze whose
/// day has arrived. Needs-review is excluded here on purpose — it has its own
/// rule, and an entry parked for review is not a thing to nag about aging.
fn in_working_set(detail: &EntryDetail, today: NaiveDate) -> bool {
    match detail.entry.state {
        EntryState::Open => true,
        EntryState::Snoozed => snooze_lapsed(detail, today),
        _ => false,
    }
}

/// Which transition, if any, this entry crossed since the baseline.
///
/// Precedence where an entry crossed more than one boundary on the same day:
/// review first (a state change a human has to answer outranks a derived
/// threshold), then the missed date, then the two silences. One line per
/// commitment either way — the digest is a glance, and the same commitment
/// listed twice reads as two problems.
fn classify(
    detail: &EntryDetail,
    live: Option<&ActionItemRow>,
    baseline_date: NaiveDate,
    baseline_instant: &str,
    today: NaiveDate,
    aging: AgingConfig,
    quiet_after_days: u32,
) -> Option<DigestKind> {
    let entry = &detail.entry;

    // Parked for review: the one rule about a *stored* state change, so it
    // reads `updated_at` rather than re-deriving anything. Both sides are
    // RFC 3339 UTC with a `Z`, where a lexical comparison is a chronological
    // one (`.claude/rules/utc-timestamps.md`).
    if entry.state == EntryState::NeedsReview {
        return (entry.updated_at.as_str() > baseline_instant)
            .then_some(DigestKind::ParkedInReview);
    }

    if !in_working_set(detail, today) {
        return None;
    }

    // Newly overdue. The note's checkbox owns done/not-done and the note's
    // `due_date` owns the deadline, so this reads the live row and nothing
    // cached: an entry whose line has vanished has no deadline to miss.
    if let Some(row) = live {
        let crossed = ActionItemStatus::derive(row.done, row.due_date.as_deref(), today)
            == ActionItemStatus::Overdue
            && ActionItemStatus::derive(row.done, row.due_date.as_deref(), baseline_date)
                != ActionItemStatus::Overdue;
        if crossed {
            return Some(DigestKind::NewlyOverdue);
        }
    }

    let tier_at = |day: NaiveDate| {
        AgingTier::derive(
            &entry.last_mention,
            entry.last_evidence_check.as_deref(),
            day,
            aging,
        )
    };
    if tier_at(today) == AgingTier::Stale && tier_at(baseline_date) != AgingTier::Stale {
        return Some(DigestKind::WentStale);
    }

    // Gone quiet, and only for what someone else owes: this is prep
    // ammunition for the next time you speak to them, which is not a question
    // that arises about your own work. Measured from the last *mention* rather
    // than the aging anchor, because an evidence check is the app looking, not
    // the two of you talking.
    if entry.direction == Direction::Theirs {
        let quiet_now = days_since(&entry.last_mention, today)?;
        let quiet_then = days_since(&entry.last_mention, baseline_date)?;
        let threshold = i64::from(quiet_after_days);
        if quiet_now >= threshold && quiet_then < threshold {
            return Some(DigestKind::TheirsQuiet);
        }
    }

    None
}

/// Whole days from a stored date (or the date half of a timestamp) to `day`.
///
/// `None` when the anchor will not parse. Unlike [`AgingTier::derive`], which
/// reads an unparseable anchor as stale, an unreadable date here yields no
/// transition at all: the tier rule is answering "does this need attention",
/// where over-reporting is the safer error, while this one is answering "did
/// something change today", where it is not.
fn days_since(anchor: &str, day: NaiveDate) -> Option<i64> {
    let parsed = anchor
        .get(..10)
        .and_then(|prefix| NaiveDate::parse_from_str(prefix, "%Y-%m-%d").ok())?;
    Some(day.signed_duration_since(parsed).num_days())
}

/// How an item sorts: kind first, then your own work before other people's,
/// then the oldest instance of whatever the kind is about.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SortKey(u8, u8, String);

fn build(
    detail: &EntryDetail,
    live: Option<&ActionItemRow>,
    kind: DigestKind,
    notes: &HashMap<String, NoteContext>,
    today: NaiveDate,
) -> (SortKey, DigestItem) {
    let entry = &detail.entry;
    let active = detail.item_refs.iter().find(|item_ref| item_ref.active);
    let context = active.and_then(|item_ref| notes.get(&item_ref.note_id));

    let due_date = live.and_then(|row| row.due_date.clone());
    let quiet_days = days_since(&entry.last_mention, today)
        .filter(|days| *days >= 0)
        .and_then(|days| u32::try_from(days).ok());

    let mine = u8::from(entry.direction != Direction::Mine);
    let tiebreak = match kind {
        // Earliest deadline first: the one furthest past is the one that has
        // been wrong for longest.
        DigestKind::NewlyOverdue => due_date.clone().unwrap_or_default(),
        DigestKind::ParkedInReview => entry.updated_at.clone(),
        DigestKind::WentStale | DigestKind::TheirsQuiet => entry.last_mention.clone(),
    };

    let item = DigestItem {
        entry_id: entry.entry_id.clone(),
        kind,
        description: live
            .map(|row| row.description.clone())
            .unwrap_or_else(|| entry.description.clone()),
        owner: live
            .map(|row| row.owner.clone())
            .unwrap_or_else(|| entry.owner.clone()),
        project: context
            .and_then(|note| note.project.clone())
            .unwrap_or_else(|| entry.project.clone()),
        note_id: context.map(|note| note.note_id.clone()),
        note_title: context.map(|note| note.title.clone()),
        due_date,
        last_mention: match kind {
            DigestKind::WentStale | DigestKind::TheirsQuiet => Some(entry.last_mention.clone()),
            _ => None,
        },
        quiet_days: match kind {
            DigestKind::TheirsQuiet => quiet_days,
            _ => None,
        },
        review_reason: match kind {
            DigestKind::ParkedInReview => entry.review_reason.clone(),
            _ => None,
        },
    };

    (SortKey(kind.rank(), mine, tiebreak), item)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Renders the digest as the body of its vault note.
///
/// **Plain bullets, never checkboxes, and never a `## Action items` heading.**
/// See the module doc: in a `type: note` body every checkbox line is extracted
/// as one of the user's own commitments, so a checkbox here would enrol the
/// digest's own contents. The section headings below are inert to that grammar
/// (it has no sections at all), and are here for a human reading the file.
///
/// Returns an empty string for an empty digest — no note is written on a quiet
/// day, and this makes that the caller's easy check rather than a special case.
pub fn render_note_body(digest: &Digest) -> String {
    if digest.is_empty() {
        return String::new();
    }

    let mut out = format!(
        "Commitment changes since {since}. A record of what moved, not a list of things to do: \
         the Commitments view is where these are acted on.\n",
        since = digest.since
    );

    for (kind, heading) in [
        (DigestKind::NewlyOverdue, "Overdue"),
        (DigestKind::ParkedInReview, "Needs review"),
        (DigestKind::WentStale, "Went stale"),
        (DigestKind::TheirsQuiet, "Gone quiet"),
    ] {
        let section: Vec<&DigestItem> = digest
            .items
            .iter()
            .filter(|item| item.kind == kind)
            .collect();
        if section.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## {heading}\n\n"));
        for item in section {
            out.push_str(&bullet(&line_for(item)));
            out.push('\n');
        }
    }

    if digest.more > 0 {
        let plural = if digest.more == 1 {
            "change"
        } else {
            "changes"
        };
        out.push_str(&format!(
            "\n{more} further {plural} did not fit this digest. The Commitments view has all of \
             them.\n",
            more = digest.more
        ));
    }

    out
}

/// Builds the digest's vault note, returning it with the title the filename
/// is slugged from.
///
/// `type: note` rather than a type of its own: the note types are a closed set
/// mirrored in the frontmatter schema, the validator and the MCP `NoteSummary`
/// shape, and a digest is a hand-readable note in every way that set is about.
/// The provenance that *does* need saying is carried by
/// [`SourceKeyword::Digest`] instead, which costs one keyword rather than seven
/// mirrored enums.
///
/// Filed by [`Routing::Manual`], which is correct and not a workaround: a
/// routing `confidence` states how strongly the router believed a guess, and
/// nothing guessed here. The date is the local calendar day the digest
/// describes, the sanctioned date-only form
/// (`.claude/rules/utc-timestamps.md`).
pub fn build_note(digest: &Digest) -> std::result::Result<(Note, String), DigestError> {
    let title = format!("Daily digest {}", digest.date);
    let note = Note::new(
        NoteId::generate().map_err(DigestError::Id)?,
        NoteType::Note,
        Routing::Manual {
            project: DIGESTS_PROJECT.to_string(),
        },
        digest.date.clone(),
        Vec::new(),
        Source::Keyword(SourceKeyword::Digest),
        render_note_body(digest),
    )
    .map_err(DigestError::Note)?
    .with_title(Some(title.clone()));
    Ok((note, title))
}

/// What can go wrong assembling the digest's note. Both arms are failures of
/// the machine rather than of the digest: the computation itself cannot fail.
#[derive(Debug)]
pub enum DigestError {
    /// The OS entropy source refused, so no note id could be minted.
    Id(std::io::Error),
    /// The assembled note did not validate against the frontmatter schema.
    Note(crate::note::NoteError),
}

impl std::fmt::Display for DigestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DigestError::Id(source) => write!(f, "could not mint a note id: {source}"),
            DigestError::Note(source) => write!(f, "the digest note did not validate: {source}"),
        }
    }
}

impl std::error::Error for DigestError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DigestError::Id(source) => Some(source),
            DigestError::Note(source) => Some(source),
        }
    }
}

/// The sentence for one item, before it is turned into a bullet.
///
/// The clause order differs by kind because what matters differs by kind: a
/// missed deadline leads with its date, a parked claim ends with the reason a
/// human is needed (which is a sentence, so nothing may follow it), and the
/// two silence kinds lead with how long it has been.
fn line_for(item: &DigestItem) -> String {
    let owner = one_line(&item.owner);
    let description = one_line(&item.description);
    let from = item
        .note_title
        .as_ref()
        .map(|title| format!("from \"{}\"", one_line(title)));

    match item.kind {
        DigestKind::NewlyOverdue => {
            let due = item
                .due_date
                .as_ref()
                .map(|due| format!("due {}", one_line(due)));
            clauses(format!("{description} ({owner})"), [due, from])
        }
        DigestKind::ParkedInReview => {
            let mut line = clauses(format!("{description} ({owner})"), [from, None]);
            if let Some(reason) = &item.review_reason {
                line.push_str(&format!(": {}", one_line(reason)));
            }
            line
        }
        DigestKind::WentStale => {
            let mention = item
                .last_mention
                .as_ref()
                .map(|mention| format!("last mentioned {}", day_of(mention)));
            clauses(format!("{description} ({owner})"), [mention, from])
        }
        DigestKind::TheirsQuiet => {
            let quiet = item.quiet_days.map(|days| {
                let plural = if days == 1 { "day" } else { "days" };
                format!("{days} {plural} since the last mention")
            });
            clauses(format!("{owner}: {description}"), [quiet, from])
        }
    }
}

/// Appends whichever trailing clauses are present, comma-separated.
fn clauses(subject: String, tail: [Option<String>; 2]) -> String {
    tail.into_iter()
        .flatten()
        .fold(subject, |mut line, clause| {
            line.push_str(", ");
            line.push_str(&clause);
            line
        })
}

/// The date half of a stored anchor, which may be a bare day or a timestamp.
fn day_of(anchor: &str) -> String {
    anchor.get(..10).unwrap_or(anchor).to_string()
}

/// Collapses a value to a single line of single-spaced text.
///
/// Every string rendered into the note passes through here, so a description
/// carrying a newline cannot open a second line in the body — which is the
/// other half of the checkbox defence, since an injected line would bypass
/// [`bullet`] entirely.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Renders one bullet, guaranteeing it cannot be read as an action item.
///
/// The grammar's markers are the exact prefixes `- [ ] ` and `- [x] ` on the
/// trimmed line, so the one accidental composition left is a description that
/// itself begins `[`. Escaping that bracket costs nothing in rendered Markdown
/// (`\[` displays as `[`) and takes the line out of the grammar. Dropping or
/// truncating such a description would lose a real commitment's text to defend
/// against a formatting coincidence.
fn bullet(text: &str) -> String {
    match text.strip_prefix('[') {
        Some(rest) => format!("- \\[{rest}"),
        None => format!("- {text}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::ledger::{EnrolledVia, ItemRef, LedgerEntry};
    use crate::meeting::derive_meeting_facts;
    use crate::note::INBOX;

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("test date parses")
    }

    /// Runs the digest body through the *production* extraction path, exactly
    /// as the index does when the note lands in the vault. Returns
    /// `(decisions, action items)`; both must be empty for every digest.
    fn facts_from(body: &str, date: &str) -> (Vec<String>, Vec<crate::meeting::ActionItemFact>) {
        let facts = derive_meeting_facts(
            "n_digest",
            NoteType::Note,
            date,
            &Source::Keyword(SourceKeyword::Digest),
            body,
            Path::new(""),
        );
        (facts.decisions, facts.action_items)
    }

    fn entry(id: &str) -> LedgerEntry {
        LedgerEntry {
            entry_id: id.to_string(),
            state: EntryState::Open,
            direction: Direction::Mine,
            owner: "You".to_string(),
            description: "Send the revised quote".to_string(),
            project: "Briarwood".to_string(),
            created_at: "2026-07-01T09:00:00Z".to_string(),
            updated_at: "2026-07-01T09:00:00Z".to_string(),
            last_mention: "2026-08-01T09:00:00Z".to_string(),
            last_evidence_check: None,
            snoozed_until: None,
            closed_via: None,
            review_reason: None,
            enrolled_via: EnrolledVia::Default,
            untracked_via: None,
            touched: false,
        }
    }

    fn detail(entry: LedgerEntry) -> EntryDetail {
        EntryDetail {
            entry,
            item_refs: Vec::new(),
            evidence: Vec::new(),
            links_out: Vec::new(),
            links_in: Vec::new(),
        }
    }

    /// An entry wired to a live source line in note `n_src`.
    fn linked(
        entry: LedgerEntry,
        due: Option<&str>,
        done: bool,
    ) -> (EntryDetail, HashMap<String, NoteContext>) {
        let entry_id = entry.entry_id.clone();
        let description = entry.description.clone();
        let owner = entry.owner.clone();
        let mut detail = detail(entry);
        detail.item_refs.push(ItemRef {
            entry_id,
            item_id: "a_1".to_string(),
            note_id: "n_src".to_string(),
            active: true,
            linked_at: "2026-07-01T09:00:00Z".to_string(),
            retired_at: None,
        });
        let mut notes = HashMap::new();
        notes.insert(
            "n_src".to_string(),
            NoteContext {
                note_id: "n_src".to_string(),
                title: "Briarwood kickoff".to_string(),
                project: Some("Briarwood".to_string()),
                path: "Briarwood/briarwood-kickoff.md".to_string(),
                category: None,
                items: vec![ActionItemRow {
                    id: "a_1".to_string(),
                    description,
                    owner,
                    due_date: due.map(str::to_string),
                    done,
                    firm: true,
                    extracted_date: Some("2026-08-01".to_string()),
                }],
            },
        );
        (detail, notes)
    }

    fn run(
        details: &[EntryDetail],
        notes: &HashMap<String, NoteContext>,
        since: &str,
        today: &str,
    ) -> Digest {
        compute(
            details,
            notes,
            day(since),
            &format!("{since}T00:00:00Z"),
            day(today),
            AgingConfig::default(),
            DEFAULT_QUIET_AFTER_DAYS,
        )
    }

    // -- transitions fire on the crossing, and only on the crossing ---------

    #[test]
    fn a_due_date_crossed_since_the_baseline_is_newly_overdue() {
        let (detail, notes) = linked(entry("le_1"), Some("2026-08-20"), false);
        let digest = run(&[detail], &notes, "2026-08-20", "2026-08-21");
        assert_eq!(digest.items.len(), 1);
        assert_eq!(digest.items[0].kind, DigestKind::NewlyOverdue);
        assert_eq!(digest.items[0].due_date.as_deref(), Some("2026-08-20"));
    }

    #[test]
    fn an_item_that_was_already_overdue_does_not_repeat() {
        let (detail, notes) = linked(entry("le_1"), Some("2026-08-10"), false);
        // Overdue on both dates: it was news a week ago, not today.
        let digest = run(&[detail], &notes, "2026-08-20", "2026-08-21");
        assert!(digest.is_empty(), "{digest:?}");
    }

    #[test]
    fn a_done_item_is_never_overdue() {
        let (detail, notes) = linked(entry("le_1"), Some("2026-08-20"), true);
        let digest = run(&[detail], &notes, "2026-08-20", "2026-08-21");
        assert!(digest.is_empty(), "{digest:?}");
    }

    #[test]
    fn crossing_the_stale_threshold_is_reported_once() {
        let mut row = entry("le_1");
        row.last_mention = "2026-07-01T09:00:00Z".to_string();
        // Default stale threshold is 30 days: 2026-07-31 is the crossing.
        let crossing = run(
            &[detail(row.clone())],
            &HashMap::new(),
            "2026-07-30",
            "2026-07-31",
        );
        assert_eq!(crossing.items.len(), 1);
        assert_eq!(crossing.items[0].kind, DigestKind::WentStale);

        let after = run(&[detail(row)], &HashMap::new(), "2026-07-31", "2026-08-05");
        assert!(after.is_empty(), "{after:?}");
    }

    #[test]
    fn entering_needs_review_since_the_baseline_is_reported() {
        let mut row = entry("le_1");
        row.state = EntryState::NeedsReview;
        row.updated_at = "2026-08-21T10:00:00Z".to_string();
        row.review_reason = Some("a conversation reported this done (n_src)".to_string());

        let fresh = run(
            &[detail(row.clone())],
            &HashMap::new(),
            "2026-08-20",
            "2026-08-21",
        );
        assert_eq!(fresh.items.len(), 1);
        assert_eq!(fresh.items[0].kind, DigestKind::ParkedInReview);
        assert!(fresh.items[0].review_reason.is_some());

        // Parked before the baseline: still parked, no longer news.
        let stale = run(&[detail(row)], &HashMap::new(), "2026-08-22", "2026-08-23");
        assert!(stale.is_empty(), "{stale:?}");
    }

    #[test]
    fn a_commitment_of_theirs_goes_quiet_on_the_threshold_day() {
        let mut row = entry("le_1");
        row.direction = Direction::Theirs;
        row.owner = "Priya".to_string();
        row.last_mention = "2026-08-01T09:00:00Z".to_string();

        let crossing = run(
            &[detail(row.clone())],
            &HashMap::new(),
            "2026-08-10",
            "2026-08-11",
        );
        assert_eq!(crossing.items.len(), 1);
        assert_eq!(crossing.items[0].kind, DigestKind::TheirsQuiet);
        assert_eq!(crossing.items[0].quiet_days, Some(10));

        let after = run(&[detail(row)], &HashMap::new(), "2026-08-11", "2026-08-12");
        assert!(after.is_empty(), "{after:?}");
    }

    #[test]
    fn a_commitment_of_mine_never_reports_as_gone_quiet() {
        let mut row = entry("le_1");
        row.direction = Direction::Mine;
        row.last_mention = "2026-08-01T09:00:00Z".to_string();
        let digest = run(&[detail(row)], &HashMap::new(), "2026-08-10", "2026-08-11");
        assert!(digest.is_empty(), "{digest:?}");
    }

    // -- what is excluded ---------------------------------------------------

    #[test]
    fn a_snoozed_entry_is_silent_until_its_day_arrives() {
        let mut row = entry("le_1");
        row.state = EntryState::Snoozed;
        row.snoozed_until = Some("2026-09-01".to_string());
        row.last_mention = "2026-07-01T09:00:00Z".to_string();
        let digest = run(&[detail(row)], &HashMap::new(), "2026-07-30", "2026-07-31");
        assert!(digest.is_empty(), "{digest:?}");
    }

    #[test]
    fn a_terminal_entry_is_never_in_the_digest() {
        for state in [
            EntryState::Closed,
            EntryState::Waived,
            EntryState::Superseded,
            EntryState::Untracked,
        ] {
            let mut row = entry("le_1");
            row.state = state;
            row.last_mention = "2026-07-01T09:00:00Z".to_string();
            let digest = run(&[detail(row)], &HashMap::new(), "2026-07-30", "2026-07-31");
            assert!(digest.is_empty(), "{state:?} leaked into the digest");
        }
    }

    #[test]
    fn a_baseline_at_or_after_today_yields_nothing() {
        let (detail, notes) = linked(entry("le_1"), Some("2026-08-20"), false);
        let digest = run(&[detail], &notes, "2026-08-21", "2026-08-21");
        assert!(digest.is_empty(), "{digest:?}");
    }

    // -- precedence, ranking, cap ------------------------------------------

    #[test]
    fn review_outranks_a_deadline_crossed_on_the_same_day() {
        let mut row = entry("le_1");
        row.state = EntryState::NeedsReview;
        row.updated_at = "2026-08-21T10:00:00Z".to_string();
        let (detail, notes) = linked(row, Some("2026-08-20"), false);
        let digest = run(&[detail], &notes, "2026-08-20", "2026-08-21");
        assert_eq!(digest.items.len(), 1, "one line per commitment");
        assert_eq!(digest.items[0].kind, DigestKind::ParkedInReview);
    }

    #[test]
    fn overdue_leads_and_mine_sorts_before_theirs() {
        let mut mine = entry("le_mine");
        mine.direction = Direction::Mine;
        let (mine_detail, notes) = linked(mine, Some("2026-08-20"), false);

        let mut theirs = entry("le_theirs");
        theirs.direction = Direction::Theirs;
        theirs.owner = "Priya".to_string();
        theirs.last_mention = "2026-08-01T09:00:00Z".to_string();
        let theirs_detail = detail(theirs);

        let mut stale = entry("le_stale");
        stale.last_mention = "2026-07-01T09:00:00Z".to_string();
        let stale_detail = detail(stale);

        // Deliberately out of rank order on the way in.
        let digest = compute(
            &[theirs_detail, stale_detail, mine_detail],
            &notes,
            day("2026-07-30"),
            "2026-07-30T00:00:00Z",
            day("2026-08-21"),
            AgingConfig::default(),
            DEFAULT_QUIET_AFTER_DAYS,
        );
        let kinds: Vec<DigestKind> = digest.items.iter().map(|item| item.kind).collect();
        assert_eq!(
            kinds,
            vec![
                DigestKind::NewlyOverdue,
                DigestKind::WentStale,
                DigestKind::TheirsQuiet
            ]
        );
    }

    #[test]
    fn the_cap_holds_and_counts_what_it_dropped() {
        let details: Vec<EntryDetail> = (0..8)
            .map(|index| {
                let mut row = entry(&format!("le_{index}"));
                row.last_mention = "2026-07-01T09:00:00Z".to_string();
                detail(row)
            })
            .collect();
        let digest = run(&details, &HashMap::new(), "2026-07-30", "2026-07-31");
        assert_eq!(digest.items.len(), DIGEST_CAP);
        assert_eq!(digest.more, 3);
    }

    // -- the trap -----------------------------------------------------------

    #[test]
    fn a_rendered_digest_note_derives_no_action_items() {
        let mut theirs = entry("le_theirs");
        theirs.direction = Direction::Theirs;
        theirs.owner = "Priya".to_string();
        theirs.last_mention = "2026-08-01T09:00:00Z".to_string();

        let mut review = entry("le_review");
        review.state = EntryState::NeedsReview;
        review.updated_at = "2026-08-11T10:00:00Z".to_string();
        review.review_reason = Some("a conversation reported this done (n_src)".to_string());

        let mut stale = entry("le_stale");
        stale.last_mention = "2026-07-12T09:00:00Z".to_string();

        let (overdue, notes) = linked(entry("le_overdue"), Some("2026-08-10"), false);
        let digest = compute(
            &[overdue, detail(theirs), detail(review), detail(stale)],
            &notes,
            day("2026-08-10"),
            "2026-08-10T00:00:00Z",
            day("2026-08-11"),
            AgingConfig::default(),
            DEFAULT_QUIET_AFTER_DAYS,
        );
        assert!(!digest.is_empty(), "the fixture should produce a digest");

        let body = render_note_body(&digest);
        // The grammar reads checkboxes anywhere in a plain note, so the body
        // must contain none, under any heading.
        assert!(
            !body.contains("- ["),
            "digest body rendered a checkbox:\n{body}"
        );
        assert!(
            !body.contains("## Action items"),
            "digest body rendered an action-items section:\n{body}"
        );

        let (decisions, items) = facts_from(&body, "2026-08-11");
        assert!(
            items.is_empty(),
            "the digest note enrolled its own contents: {items:?}"
        );
        assert!(decisions.is_empty(), "{decisions:?}");
    }

    #[test]
    fn a_description_that_looks_like_a_checkbox_is_escaped() {
        let mut row = entry("le_1");
        row.description = "[ ] sneaky".to_string();
        row.last_mention = "2026-07-01T09:00:00Z".to_string();
        let digest = run(&[detail(row)], &HashMap::new(), "2026-07-30", "2026-07-31");

        let body = render_note_body(&digest);
        assert!(body.contains("- \\[ ] sneaky"), "{body}");
        let (_, items) = facts_from(&body, "2026-07-31");
        assert!(
            items.is_empty(),
            "an escaped description still enrolled: {items:?}"
        );
    }

    #[test]
    fn a_multi_line_description_cannot_open_a_second_line() {
        let mut row = entry("le_1");
        row.description = "first\n- [ ] injected".to_string();
        row.last_mention = "2026-07-01T09:00:00Z".to_string();
        let digest = run(&[detail(row)], &HashMap::new(), "2026-07-30", "2026-07-31");

        let body = render_note_body(&digest);
        let (_, items) = facts_from(&body, "2026-07-31");
        assert!(items.is_empty(), "an injected line enrolled: {items:?}");
    }

    #[test]
    fn the_body_carries_no_em_dash() {
        let mut row = entry("le_1");
        row.last_mention = "2026-07-01T09:00:00Z".to_string();
        let digest = run(&[detail(row)], &HashMap::new(), "2026-07-30", "2026-07-31");
        let body = render_note_body(&digest);
        assert!(!body.contains('\u{2014}'), "{body}");
    }

    /// The rendered note in full, so the copy is reviewable in a diff rather
    /// than only inferable from the format strings.
    #[test]
    fn the_rendered_body_reads_as_written() {
        let mut theirs = entry("le_theirs");
        theirs.direction = Direction::Theirs;
        theirs.owner = "Priya".to_string();
        theirs.description = "the vendor shortlist".to_string();
        theirs.last_mention = "2026-08-01T09:00:00Z".to_string();

        let mut review = entry("le_review");
        review.state = EntryState::NeedsReview;
        review.description = "Confirm the audit hand-off".to_string();
        review.updated_at = "2026-08-11T10:00:00Z".to_string();
        review.review_reason = Some("a conversation reported this done".to_string());

        let mut stale = entry("le_stale");
        stale.description = "Draft the Q4 brief".to_string();
        // 30 days before `today`, so the stale threshold is crossed on the day
        // the digest runs and not before it.
        stale.last_mention = "2026-07-12T09:00:00Z".to_string();

        let (overdue, notes) = linked(entry("le_overdue"), Some("2026-08-10"), false);
        let digest = compute(
            &[overdue, detail(theirs), detail(review), detail(stale)],
            &notes,
            day("2026-08-10"),
            "2026-08-10T00:00:00Z",
            day("2026-08-11"),
            AgingConfig::default(),
            DEFAULT_QUIET_AFTER_DAYS,
        );

        assert_eq!(
            render_note_body(&digest),
            "Commitment changes since 2026-08-10. A record of what moved, not a list of things \
             to do: the Commitments view is where these are acted on.\n\
             \n## Overdue\n\n\
             - Send the revised quote (You), due 2026-08-10, from \"Briarwood kickoff\"\n\
             \n## Needs review\n\n\
             - Confirm the audit hand-off (You): a conversation reported this done\n\
             \n## Went stale\n\n\
             - Draft the Q4 brief (You), last mentioned 2026-07-12\n\
             \n## Gone quiet\n\n\
             - Priya: the vendor shortlist, 10 days since the last mention\n"
        );
    }

    #[test]
    fn the_built_note_validates_and_carries_the_digest_provenance() {
        let mut row = entry("le_1");
        row.last_mention = "2026-07-12T09:00:00Z".to_string();
        let digest = run(&[detail(row)], &HashMap::new(), "2026-08-10", "2026-08-11");

        let (note, title) = build_note(&digest).expect("a digest note validates");
        assert_eq!(title, "Daily digest 2026-08-11");
        assert_eq!(note.note_type, NoteType::Note);
        assert_eq!(note.routing.project(), DIGESTS_PROJECT);
        assert_eq!(note.routing.confidence(), None);
        assert_eq!(note.date, "2026-08-11");
        assert_eq!(note.source, Source::Keyword(SourceKeyword::Digest));

        // The emitted frontmatter, minus the minted id: this is what the
        // validator fixture at
        // `.claude/skills/frontmatter-validator/fixtures/valid/daily-digest.md`
        // mirrors, and pinning it here is what stops the two drifting.
        let markdown = note.to_markdown();
        let frontmatter: Vec<&str> = markdown
            .lines()
            .skip(1)
            .take_while(|line| *line != "---")
            .filter(|line| !line.starts_with("id: "))
            .collect();
        assert_eq!(
            frontmatter,
            vec![
                "type: note",
                "title: Daily digest 2026-08-11",
                "project: Digests",
                "date: 2026-08-11",
                "source: digest",
            ]
        );

        let (_, items) = facts_from(&note.body, &note.date);
        assert!(items.is_empty(), "{items:?}");
    }

    #[test]
    fn an_empty_digest_renders_no_body() {
        let digest = Digest::empty(day("2026-08-21"), day("2026-08-20"));
        assert!(render_note_body(&digest).is_empty());
    }

    #[test]
    fn the_project_falls_back_to_the_entrys_own_when_the_note_is_gone() {
        let mut row = entry("le_1");
        row.project = INBOX.to_string();
        row.last_mention = "2026-07-01T09:00:00Z".to_string();
        let digest = run(&[detail(row)], &HashMap::new(), "2026-07-30", "2026-07-31");
        assert_eq!(digest.items[0].project, INBOX);
        assert_eq!(digest.items[0].note_id, None);
    }
}
