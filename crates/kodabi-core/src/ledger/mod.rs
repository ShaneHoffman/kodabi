//! The commitment ledger — durable state, evidence, and lifecycle over the
//! action items the distill pass already extracts.
//!
//! **This database is not a cache.** Unlike [`crate::index`], which is a
//! rebuildable derivation of the Markdown corpus and may be nuked at any time,
//! the ledger holds judgements a human made that exist nowhere else: a waiver, a
//! snooze, a closure with its provenance, the link that says two meetings named
//! one commitment. Deleting `ledger.db` loses those, so it lives beside
//! `settings.toml` in the config dir rather than inside the index, its
//! migrations must carry data forward (never drop-and-recreate), and every
//! change is mirrored into a per-project YAML snapshot in the vault
//! ([`snapshot`]) so "back up the vault" stays the whole backup story.
//!
//! **The note's checkbox stays the source of truth for done/not-done.** Nothing
//! here stores `done`, and nothing here derives it: a `- [x]` flip is invisible
//! to the ledger by design. What is stored is only what Markdown cannot carry —
//! identity across edits, evidence from outside the vault, and the states a
//! checkbox has no spelling for.
//!
//! ## Identity, and why entries have their own ids
//!
//! An extracted item's `a_` id is a hash of the *parsed* line
//! ([`crate::meeting::ActionItemFact`]), so it survives a checkbox flip but is
//! re-minted the moment the owner, description, or due date is edited — and the
//! per-line occurrence counter means deleting one of two identical lines hands
//! its id to the survivor. An `a_` id is therefore a **re-linkable reference**,
//! never a durable key. Entries carry their own random `le_` id and reference
//! items through [`ledger_item_refs`](migrations), whose rows retire rather than
//! disappear. [`sync`] is where a re-minted id is re-attached to its entry.

pub mod commitments;
pub mod distill_apply;
mod enrollment;
mod migrations;
pub mod snapshot;
mod store;
mod sync;
pub mod view;

pub use distill_apply::{apply_distill_follow_up, AppliedUpdates, AutoClose, DistillFollowUp};
pub use enrollment::{DrainOutcome, NoteTrackingOutcome, OwnerResolutionOutcome, RetroSource};
pub use snapshot::{ProjectSnapshot, RestoreReport, LEDGER_SNAPSHOT_FILE, LEDGER_SNAPSHOT_VERSION};
pub use store::{
    EntryDetail, EntryFilter, EntryLink, Evidence, ItemRef, LedgerEntry, TRIAGE_LAST_SEEN_KEY,
};
pub use sync::{LinkHint, NoteSync, SyncOutcome};
pub use view::{
    mention_window_cutoff, AgingConfig, AgingTier, Commitment, CommitmentItem, CommitmentSource,
    ItemTracking, NoteContext, NoteItemEnrollment, DEFAULT_AGING_AFTER_DAYS,
    DEFAULT_STALE_AFTER_DAYS,
};

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::BUSY_TIMEOUT_MS;

/// Filename of the ledger database, resolved against the app config dir (the
/// shell's `sandbox::config_dir`, so `KODABI_SANDBOX` relocates it with the rest
/// of the config state).
pub const LEDGER_DB_FILE: &str = "ledger.db";

/// How sure a conversation's report that a commitment was already done has to
/// be before the app closes it unasked.
///
/// The default sits high on purpose: the two failures are not symmetric. A
/// claim parked for review costs one click; a commitment closed wrongly is one
/// the user stops seeing and never delivers. Below this, the evidence is still
/// recorded and the entry parks in [`EntryState::NeedsReview`] rather than
/// being discarded.
pub const DEFAULT_CONVERSATION_AUTOCLOSE: f64 = 0.8;

/// Base36 alphabet for minted ids (mirrors [`crate::note`]'s).
const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Random suffix length for a minted id. Longer than a note id's 8 because the
/// ledger mints ids far more often (one per commitment *and* one per evidence
/// claim) and, unlike notes, has no filesystem collision check behind it.
const ID_RANDOM_LEN: usize = 12;

/// Minimum random-suffix length a parsed id may carry, so a future shortening
/// never invalidates ids already in the field.
const ID_MIN_RANDOM_LEN: usize = 6;

/// Errors from opening, migrating, querying, or snapshotting the ledger.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// Any failure surfaced by SQLite (open, migrate, query, constraint).
    #[error("ledger database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A snapshot file could not be read, written, or renamed.
    #[error("ledger snapshot I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// A snapshot file's YAML could not be parsed or rendered.
    #[error("ledger snapshot YAML error at {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
    /// No entry with this id.
    #[error("no ledger entry {entry_id:?}")]
    EntryNotFound { entry_id: String },
    /// No evidence claim with this id on the entry it was looked up against.
    #[error("no ledger evidence {evidence_id:?}")]
    EvidenceNotFound { evidence_id: String },
    /// A lifecycle transition the state machine forbids (see
    /// [`store::LedgerEntry`]'s transition table).
    #[error("illegal ledger transition {from} -> {to} for {entry_id:?}")]
    IllegalTransition {
        entry_id: String,
        from: EntryState,
        to: EntryState,
    },
    /// A caller-supplied field failed validation before it reached SQLite.
    #[error("invalid {field}: {detail}")]
    InvalidField { field: &'static str, detail: String },
    /// The OS entropy source was unavailable while minting an id.
    #[error("OS RNG unavailable: {0}")]
    Rng(String),
}

/// `Result` specialised to [`LedgerError`].
pub type Result<T> = std::result::Result<T, LedgerError>;

/// Builds an [`LedgerError::InvalidField`].
pub(crate) fn invalid_field(field: &'static str, detail: impl Into<String>) -> LedgerError {
    LedgerError::InvalidField {
        field,
        detail: detail.into(),
    }
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// Where an entry sits in its lifecycle.
///
/// Deliberately *not* a done/not-done axis: the note's checkbox owns that. These
/// are the states a checkbox cannot spell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    /// Live and tracked.
    Open,
    /// The entry lost its source line (edited beyond recognition, or the note
    /// was deleted) and needs a human to say what happened.
    NeedsReview,
    /// Resolved, with provenance in [`LedgerEntry::closed_via`]. A stronger
    /// claim than a checked box: it survives the line being edited away.
    Closed,
    /// Replaced by another entry; set only by [`Ledger::link_entries`] so the
    /// state and the link can never drift apart.
    Superseded,
    /// Deliberately not going to happen, and that is fine.
    Waived,
    /// Out of sight until [`LedgerEntry::snoozed_until`]. Expiry is evaluated at
    /// read time; nothing writes on the day it lapses.
    Snoozed,
    /// Never belonged in the working set — distinct from [`Waived`], which says
    /// it was mine and stopped being relevant. Provenance in
    /// [`LedgerEntry::untracked_via`] separates a person's untrack from one a
    /// meeting's tracking override applied.
    ///
    /// Its item refs stay **active**: an untracked entry still owns its line,
    /// so a re-sync of the note hits [`sync`]'s present leg and can neither
    /// mint a duplicate nor park it in
    /// [`NeedsReview`](EntryState::NeedsReview) as vanished. (Retiring a ref is
    /// always about the *line* going away, never about the entry's state, so
    /// this is what every exit does — it is called out here because the
    /// duplicate it prevents is one only this state could produce:
    /// `match_live_entry` skips untracked entries, so a line that lost its ref
    /// would be re-minted rather than re-matched.)
    ///
    /// [`Waived`]: EntryState::Waived
    Untracked,
}

impl EntryState {
    /// The stored spelling, shared by SQLite's `CHECK` and the YAML snapshot.
    pub fn as_str(self) -> &'static str {
        match self {
            EntryState::Open => "open",
            EntryState::NeedsReview => "needs_review",
            EntryState::Closed => "closed",
            EntryState::Superseded => "superseded",
            EntryState::Waived => "waived",
            EntryState::Snoozed => "snoozed",
            EntryState::Untracked => "untracked",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "open" => Ok(EntryState::Open),
            "needs_review" => Ok(EntryState::NeedsReview),
            "closed" => Ok(EntryState::Closed),
            "superseded" => Ok(EntryState::Superseded),
            "waived" => Ok(EntryState::Waived),
            "snoozed" => Ok(EntryState::Snoozed),
            "untracked" => Ok(EntryState::Untracked),
            other => Err(invalid_field(
                "state",
                format!("unknown entry state {other:?}"),
            )),
        }
    }

    /// Whether this state is terminal — reachable back to `Open` only through
    /// a deliberate act ([`Ledger::reopen`], [`Ledger::unlink_entries`],
    /// [`Ledger::track_item`], or a tracking flip), never by sync.
    ///
    /// [`Untracked`](EntryState::Untracked) counts, and that one word buys three
    /// behaviours: sync's vanish leg leaves it alone, a distill classification
    /// naming it is skipped as already settled, and a link hint pointing at it
    /// falls through to a fresh entry.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            EntryState::Closed
                | EntryState::Superseded
                | EntryState::Waived
                | EntryState::Untracked
        )
    }
}

impl fmt::Display for EntryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which way a commitment points. Both directions are first-class: "waiting on
/// them" is the half a checkbox list normally loses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// The local user took it on.
    Mine,
    /// Someone else did.
    Theirs,
    /// The line named no owner.
    Unassigned,
}

impl Direction {
    /// Derives the direction from the grammar's owner string, knowing nothing
    /// about who the user is.
    ///
    /// `"You"` is the distill prompt's convention for the local user
    /// ([`crate::distill`]), and [`crate::distill::UNASSIGNED_OWNER`] its
    /// sentinel for an unattributed line. Everything else is a named other.
    /// Case-insensitive: the owner is free text and only the renderer
    /// capitalizes it.
    ///
    /// This is [`Direction::resolve`] against an empty [`OwnerIdentity`], and
    /// exists for the callers that genuinely have no identity to offer (tests,
    /// and reads that only care whether a line said `Unassigned`). A path that
    /// enrols or re-files a commitment wants `resolve`: a user who has told the
    /// app their name expects `"Avery to send the deck"` to be theirs.
    pub fn from_owner(owner: &str) -> Direction {
        Direction::resolve(owner, &OwnerIdentity::default())
    }

    /// Derives the direction from the owner string, resolving the user's own
    /// names to [`Direction::Mine`].
    ///
    /// The grammar's `"You"` always wins first: it is the prompt's own spelling
    /// for the local user, so it means "me" whether or not a name is
    /// configured, and no alias can redefine it. `Unassigned` is likewise
    /// reserved — it exists precisely so an unattributed line does *not*
    /// silently become the user's, and an alias set must never swallow it.
    ///
    /// Everything else falls to the identity, and an unresolved name is
    /// **theirs**. That asymmetry is deliberate and matches the enrolment
    /// gate's observer rule: a stray sitting in Waiting-on-them is one click to
    /// fix, while a commitment wrongly filed as the user's own is a promise
    /// they never made.
    pub fn resolve(owner: &str, identity: &OwnerIdentity) -> Direction {
        let owner = owner.trim();
        if owner.eq_ignore_ascii_case("you") {
            Direction::Mine
        } else if owner.eq_ignore_ascii_case(crate::distill::UNASSIGNED_OWNER) {
            Direction::Unassigned
        } else if identity.is_me(owner) {
            Direction::Mine
        } else {
            Direction::Theirs
        }
    }

    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Mine => "mine",
            Direction::Theirs => "theirs",
            Direction::Unassigned => "unassigned",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "mine" => Ok(Direction::Mine),
            "theirs" => Ok(Direction::Theirs),
            "unassigned" => Ok(Direction::Unassigned),
            other => Err(invalid_field(
                "direction",
                format!("unknown direction {other:?}"),
            )),
        }
    }
}

/// The set of owner spellings that mean the local user.
///
/// Built from [`crate::settings::IdentitySettings`] — display name and aliases
/// flattened together — and matched with [`normalize_owner`], the same
/// normalization that produced the stored `owner_norm` column, so a name
/// matched here and a name matched by [`sync`] can never disagree.
///
/// **Normalization, not fuzzy matching.** Case, surrounding and interior
/// whitespace, and the two Unicode spellings of an accented name are folded;
/// nothing else is. No prefix matching (`"Avery"` must not claim `"Avery Kim"`
/// unless the user said so), no edit distance, no first-name-of guessing —
/// every one of those trades a silent, unexplainable mis-filing for
/// convenience the claim affordance already provides in one click. A miss is
/// cheap and self-correcting; a false positive puts words in the user's mouth.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerIdentity {
    aliases: BTreeSet<String>,
}

impl OwnerIdentity {
    /// Builds the matcher from a display name and its other spellings. Blank
    /// entries are dropped, so an unconfigured identity is simply empty and
    /// every owner resolves the way it did before there was an identity at all.
    pub fn new(display_name: &str, aliases: &[String]) -> OwnerIdentity {
        let aliases = std::iter::once(display_name)
            .chain(aliases.iter().map(String::as_str))
            .map(normalize_owner)
            .filter(|alias| !alias.is_empty())
            .collect();
        OwnerIdentity { aliases }
    }

    /// Whether `owner` is one of the user's names.
    ///
    /// Blind to `"You"` and `Unassigned` on purpose: those are the grammar's
    /// tokens, decided by [`Direction::resolve`] before it ever asks here.
    pub fn is_me(&self, owner: &str) -> bool {
        let owner = normalize_owner(owner);
        !owner.is_empty() && self.aliases.contains(&owner)
    }

    /// Whether no name has been configured. An empty identity resolves owners
    /// exactly as [`Direction::from_owner`] does.
    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }

    /// The normalized spellings, for the sweep's `owner_norm` lookup.
    pub(crate) fn normalized_aliases(&self) -> &BTreeSet<String> {
        &self.aliases
    }
}

/// Normalizes an owner string for matching: NFC, lowercase, whitespace runs
/// collapsed, trimmed.
///
/// The public face of the normalization behind the `owner_norm` column, so
/// callers outside this module (settings de-duplicating a learned alias) fold
/// names the exact same way the database does.
pub fn normalize_owner(owner: &str) -> String {
    store::normalize(owner)
}

/// The owner string a "this is me" claim may learn as an alias, or `None` when
/// the spelling is one the app already owns.
///
/// Three spellings are refused. `"You"` and [`crate::distill::UNASSIGNED_OWNER`]
/// are the grammar's own tokens, already resolved before any alias is
/// consulted; learning them would be a no-op at best. `"Them"` is the sharper
/// case: the distill guidance writes it for a commitment the *other* side took
/// when nobody named the speaker, so adopting it as one of the user's names
/// would quietly claim every future unnamed them-side commitment — the exact
/// failure the unresolved-defaults-to-theirs rule exists to prevent.
pub fn learnable_alias(owner: &str) -> Option<&str> {
    let owner = owner.trim();
    if owner.is_empty()
        || owner.eq_ignore_ascii_case("you")
        || owner.eq_ignore_ascii_case("them")
        || owner.eq_ignore_ascii_case(crate::distill::UNASSIGNED_OWNER)
    {
        return None;
    }
    Some(owner)
}

/// How a closure was reached. Source-agnostic by design: GitHub and conversation
/// are simply the first two evidence providers, and adding one (ADO, Linear) is
/// a new variant plus a migration, not a new model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosedVia {
    /// A human said so.
    Manual,
    /// Inferred from a distilled conversation.
    Conversation,
    /// Observed in a GitHub artifact (a merged PR, a closed issue).
    Github,
}

impl ClosedVia {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ClosedVia::Manual => "manual",
            ClosedVia::Conversation => "conversation",
            ClosedVia::Github => "github",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "manual" => Ok(ClosedVia::Manual),
            "conversation" => Ok(ClosedVia::Conversation),
            "github" => Ok(ClosedVia::Github),
            other => Err(invalid_field(
                "closed_via",
                format!("unknown closure provenance {other:?}"),
            )),
        }
    }
}

/// Where an evidence claim came from. Mirrors [`ClosedVia`] deliberately: a
/// closure's provenance is the source of the claim that closed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSource {
    Manual,
    Conversation,
    Github,
}

impl EvidenceSource {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceSource::Manual => "manual",
            EvidenceSource::Conversation => "conversation",
            EvidenceSource::Github => "github",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "manual" => Ok(EvidenceSource::Manual),
            "conversation" => Ok(EvidenceSource::Conversation),
            "github" => Ok(EvidenceSource::Github),
            other => Err(invalid_field(
                "source",
                format!("unknown evidence source {other:?}"),
            )),
        }
    }

    /// The closure provenance this source implies, for a caller closing an entry
    /// on the strength of one claim.
    pub fn as_closed_via(self) -> ClosedVia {
        match self {
            EvidenceSource::Manual => ClosedVia::Manual,
            EvidenceSource::Conversation => ClosedVia::Conversation,
            EvidenceSource::Github => ClosedVia::Github,
        }
    }
}

/// How one entry relates to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// The commitment changed into a different one.
    Supersedes,
    /// The same commitment, re-stated later; the two are merged by hand.
    Refreshes,
}

impl LinkKind {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Supersedes => "supersedes",
            LinkKind::Refreshes => "refreshes",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "supersedes" => Ok(LinkKind::Supersedes),
            "refreshes" => Ok(LinkKind::Refreshes),
            other => Err(invalid_field(
                "kind",
                format!("unknown link kind {other:?}"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Enrollment
// ---------------------------------------------------------------------------

/// Whether a meeting's extracted items become tracked entries.
///
/// **Extraction is not tracking.** The distill pass always records what was
/// said, and the note, the index, and the MCP read surface are identical either
/// way. This decides only whether an extracted line earns a ledger entry — and
/// an item with no entry is invisible to the aging and evidence passes, which
/// is the whole point: a meeting attended for context should not fill the
/// ledger with other people's commitments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrollmentMode {
    /// Every extracted item is enrolled. The global default.
    Tracked,
    /// Only items the local user owns ([`Direction::Mine`]) are enrolled.
    ///
    /// A direct ask is a commitment regardless of why you attended: "Shane, can
    /// you send us that deck" is yours whether or not you were there to listen.
    /// Everything else stays in the note and out of the working set.
    ContextOnly,
}

impl EnrollmentMode {
    /// The stored spelling, shared by SQLite's `CHECK` and the YAML snapshot.
    pub fn as_str(self) -> &'static str {
        match self {
            EnrollmentMode::Tracked => "tracked",
            EnrollmentMode::ContextOnly => "context_only",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "tracked" => Ok(EnrollmentMode::Tracked),
            "context_only" => Ok(EnrollmentMode::ContextOnly),
            other => Err(invalid_field(
                "mode",
                format!("unknown enrollment mode {other:?}"),
            )),
        }
    }

    /// The frontmatter spelling, which is kebab-case rather than the snake_case
    /// [`EnrollmentMode::as_str`] above.
    ///
    /// The note's `tracking:` key is read by humans in Obsidian and sits beside
    /// `source: quick-capture` and kebab-case tags, so it follows the schema's
    /// house style; SQLite's `CHECK`, the YAML snapshot and this enum's serde
    /// representation keep the snake_case spelling they were written with. The
    /// two spellings never meet: this pair is the only bridge.
    pub fn as_frontmatter_str(self) -> &'static str {
        match self {
            EnrollmentMode::Tracked => "tracked",
            EnrollmentMode::ContextOnly => "context-only",
        }
    }

    /// Parses the frontmatter spelling (the inverse of
    /// [`EnrollmentMode::as_frontmatter_str`]).
    pub fn parse_frontmatter(raw: &str) -> Result<Self> {
        match raw {
            "tracked" => Ok(EnrollmentMode::Tracked),
            "context-only" => Ok(EnrollmentMode::ContextOnly),
            other => Err(invalid_field(
                "tracking",
                format!("tracking {other:?} must be one of tracked | context-only"),
            )),
        }
    }

    /// Whether `direction` enrolls under this mode.
    pub fn enrolls(self, direction: Direction) -> bool {
        match self {
            EnrollmentMode::Tracked => true,
            EnrollmentMode::ContextOnly => direction == Direction::Mine,
        }
    }
}

/// Resolves the enrollment mode that applies to one meeting.
///
/// **This function is the seam,** and it is one chain rather than two: the
/// per-meeting override, then the meeting category's default, then the global
/// default of [`EnrollmentMode::Tracked`].
///
/// The per-meeting override arrives from the note's frontmatter `tracking:` key
/// (via [`sync::NoteSync::note_override`]); the category default arrives from
/// the settings the shell resolved with [`category_default_for`], because this
/// store can read neither the vault nor the settings file. Both are inputs for
/// the same reason, and both are un-omittable fields on a struct with no
/// `Default` so a producer cannot silently drop half the chain.
pub fn effective_mode(
    note_override: Option<EnrollmentMode>,
    category_default: Option<EnrollmentMode>,
) -> EnrollmentMode {
    note_override
        .or(category_default)
        .unwrap_or(EnrollmentMode::Tracked)
}

/// The enrollment default a meeting category carries when the user has not set
/// one — the product's opinion about what each genre is *for*.
///
/// Two genres are attended rather than transacted: an `observer` sitting-in and
/// an `all-hands` broadcast produce plenty of other people's commitments and
/// almost none of yours, which is exactly the noise
/// [`EnrollmentMode::ContextOnly`] exists to keep out. Everything else is a room
/// you are working in, so it tracks in full.
///
/// The match is exhaustive on purpose: a new genre does not compile until
/// someone decides which side of that line it falls on.
pub fn builtin_category_default(category: crate::note::MeetingCategory) -> EnrollmentMode {
    use crate::note::MeetingCategory as Category;
    match category {
        Category::AllHands | Category::Observer => EnrollmentMode::ContextOnly,
        Category::Standup
        | Category::OneOnOne
        | Category::Client
        | Category::WorkingSession
        | Category::Review => EnrollmentMode::Tracked,
    }
}

/// The category slot of [`effective_mode`], resolved for one note.
///
/// `None` means the note has no category to inherit from (a chat, or a meeting
/// the classifier left unlabelled), so the chain falls straight through to the
/// global default.
///
/// **A stored `None` on the category's prefs means "inherit the builtin", not
/// "track".** That is what makes [`builtin_category_default`] reach installs
/// whose settings file predates this wiring: every field file already carries
/// `[categories.*]` tables whose `enrollment_default` is absent, and serde fills
/// those with `None` rather than with any hand-written default. A user who wants
/// an all-hands tracked in full stores `Some(Tracked)`, which overrules the
/// builtin exactly as it reads.
pub fn category_default_for(
    category: Option<crate::note::MeetingCategory>,
    categories: &crate::settings::CategorySettings,
) -> Option<EnrollmentMode> {
    category.map(|category| {
        categories
            .for_category(category)
            .enrollment_default
            .unwrap_or_else(|| builtin_category_default(category))
    })
}

/// Why an entry is in the ledger — the answer to "why are you in my working
/// set", which before this existed no row could give.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnrolledVia {
    /// Enrolled because nothing said otherwise: the global default.
    #[default]
    Default,
    /// Enrolled under a meeting whose tracking override was set — so, a
    /// [`Direction::Mine`] item in a context-only meeting.
    Override,
    /// Promoted by hand from the note, against whatever the mode said.
    Manual,
    /// Enrolled under a meeting whose *category* gates — so, a
    /// [`Direction::Mine`] item in an all-hands or an observer meeting that
    /// carries no override of its own.
    ///
    /// Distinct from [`EnrolledVia::Override`] because the judgement is a
    /// standing one about a genre rather than a decision about this meeting, and
    /// recategorizing may revisit it where flipping the override may not.
    Category,
}

impl EnrolledVia {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            EnrolledVia::Default => "default",
            EnrolledVia::Override => "override",
            EnrolledVia::Manual => "manual",
            EnrolledVia::Category => "category",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "default" => Ok(EnrolledVia::Default),
            "override" => Ok(EnrolledVia::Override),
            "manual" => Ok(EnrolledVia::Manual),
            "category" => Ok(EnrolledVia::Category),
            other => Err(invalid_field(
                "enrolled_via",
                format!("unknown enrollment provenance {other:?}"),
            )),
        }
    }
}

/// How an entry left the working set, for the untracked half of the split.
///
/// The distinction is load-bearing for retro-application: flipping a meeting
/// back to tracked revives what the override untracked and leaves a person's
/// own untrack alone. Both machine reasons are revivable and a person's is not,
/// which is the whole of the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UntrackedVia {
    /// A human untracked it.
    Manual,
    /// A meeting's tracking override untracked it.
    Override,
    /// A meeting's category default untracked it, when the meeting was
    /// recategorized into a genre that tracks direct asks only.
    Category,
}

impl UntrackedVia {
    /// The stored spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            UntrackedVia::Manual => "manual",
            UntrackedVia::Override => "override",
            UntrackedVia::Category => "category",
        }
    }

    /// Parses the stored spelling.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "manual" => Ok(UntrackedVia::Manual),
            "override" => Ok(UntrackedVia::Override),
            "category" => Ok(UntrackedVia::Category),
            other => Err(invalid_field(
                "untracked_via",
                format!("unknown untrack provenance {other:?}"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Id minting
// ---------------------------------------------------------------------------

/// Mints a random id with `prefix`, e.g. `le_9k2m4p7q1r3s`.
///
/// **Random, not a content hash** — which is the whole point of the ledger. The
/// `a_` item ids are hashes, so they churn exactly when durability matters most
/// (someone edits the line). An entry's identity must outlive its text.
///
/// Rejection-sampled so every base36 symbol is equally likely (mirrors
/// [`crate::note::NoteId::generate`]).
pub(crate) fn mint_id(prefix: &str) -> Result<String> {
    const REJECT_THRESHOLD: u8 = (256 / ALPHABET.len() * ALPHABET.len()) as u8;

    let mut random = String::with_capacity(ID_RANDOM_LEN);
    let mut buf = [0u8; ID_RANDOM_LEN];
    while random.len() < ID_RANDOM_LEN {
        getrandom::getrandom(&mut buf).map_err(|err| LedgerError::Rng(err.to_string()))?;
        for &byte in &buf {
            if byte < REJECT_THRESHOLD {
                random.push(ALPHABET[(byte % ALPHABET.len() as u8) as usize] as char);
                if random.len() == ID_RANDOM_LEN {
                    break;
                }
            }
        }
    }
    Ok(format!("{prefix}{random}"))
}

/// Whether `id` is a well-formed minted id with `prefix` — used when restoring a
/// snapshot written by another machine, where the ids are inputs rather than
/// something this process minted.
pub(crate) fn is_minted_id(id: &str, prefix: &str) -> bool {
    match id.strip_prefix(prefix) {
        Some(rest) => {
            rest.len() >= ID_MIN_RANDOM_LEN && rest.bytes().all(|b| ALPHABET.contains(&b))
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

/// A handle to the commitment ledger, owning its connection.
///
/// Opening runs any pending migrations, so a returned `Ledger` is always at the
/// current schema version.
///
/// Timestamps are always **supplied by the caller** as RFC 3339 UTC with a `Z`
/// suffix and seconds precision (`.claude/rules/utc-timestamps.md`). kodabi-core
/// never reads the clock, matching the index's `today`-is-an-argument doctrine,
/// which is what makes every operation here deterministically testable.
pub struct Ledger {
    conn: Connection,
    /// Project slugs whose `_ledger.yml` no longer matches the database.
    ///
    /// In-memory and process-lifetime: the database is the truth and the
    /// snapshot is a backup, so losing this set to a crash costs only backup
    /// freshness. The shell drains it on a debounce and at exit
    /// ([`Ledger::flush_snapshots`]).
    dirty: BTreeSet<String>,
}

impl Ledger {
    /// Opens (creating if absent) the ledger database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    /// Opens a fresh in-memory ledger — for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Configures connection pragmas and migrates to the current schema.
    ///
    /// `foreign_keys` is load-bearing here rather than incidental: an entry's
    /// refs, evidence, and links cascade from it, so a delete that left them
    /// behind would resurrect as orphan rows in the next snapshot.
    ///
    /// `busy_timeout` is load-bearing too, and newer: the ledger used to have
    /// exactly one writer (the app's worker thread owns it outright), but the
    /// MCP server writes a person's mark-done judgement from its own process.
    /// rusqlite defaults the timeout to zero, so the loser of a race would get
    /// `SQLITE_BUSY` on the first contended write instead of waiting out a
    /// transaction that lasts microseconds. Set before [`migrations::apply`],
    /// so a migration racing another opener waits rather than failing.
    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = {BUSY_TIMEOUT_MS}; \
             PRAGMA foreign_keys = ON;"
        ))?;
        migrations::apply(&mut conn)?;
        Ok(Self {
            conn,
            dirty: BTreeSet::new(),
        })
    }

    /// Marks `project`'s snapshot as stale.
    pub(crate) fn mark_dirty(&mut self, project: &str) {
        if !project.is_empty() {
            self.dirty.insert(project.to_string());
        }
    }

    /// The project slugs whose snapshots are behind the database.
    pub fn dirty_projects(&self) -> Vec<String> {
        self.dirty.iter().cloned().collect()
    }

    /// Marks one project's snapshot as current again.
    pub(crate) fn clear_dirty(&mut self, project: &str) {
        self.dirty.remove(project);
    }

    /// Marks every snapshot current — the state after a restore, where the
    /// database was built *from* the files and rewriting them would be churn.
    pub(crate) fn clear_all_dirty(&mut self) {
        self.dirty.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::MeetingCategory;
    use crate::settings::{CategoryPrefs, CategorySettings};
    use tempfile::tempdir;

    #[test]
    fn two_genres_are_attended_rather_than_transacted() {
        for category in [MeetingCategory::AllHands, MeetingCategory::Observer] {
            assert_eq!(
                builtin_category_default(category),
                EnrollmentMode::ContextOnly,
                "{} is a room you sit in",
                category.as_str()
            );
        }
        for category in [
            MeetingCategory::Standup,
            MeetingCategory::OneOnOne,
            MeetingCategory::Client,
            MeetingCategory::WorkingSession,
            MeetingCategory::Review,
        ] {
            assert_eq!(
                builtin_category_default(category),
                EnrollmentMode::Tracked,
                "{} is a room you work in",
                category.as_str()
            );
        }
    }

    #[test]
    fn an_unset_preference_inherits_the_builtin_rather_than_tracking() {
        // The load-bearing case: every settings file in the field predates this
        // wiring and stores `None` for all seven genres. If `None` read as
        // "track", the built-in defaults would reach nobody but new installs.
        let settings = CategorySettings::default();
        assert_eq!(
            category_default_for(Some(MeetingCategory::AllHands), &settings),
            Some(EnrollmentMode::ContextOnly)
        );
        assert_eq!(
            category_default_for(Some(MeetingCategory::Standup), &settings),
            Some(EnrollmentMode::Tracked)
        );
    }

    #[test]
    fn a_stored_preference_overrules_the_builtin_in_both_directions() {
        let settings = CategorySettings {
            all_hands: CategoryPrefs {
                enrollment_default: Some(EnrollmentMode::Tracked),
            },
            standup: CategoryPrefs {
                enrollment_default: Some(EnrollmentMode::ContextOnly),
            },
            ..Default::default()
        };

        assert_eq!(
            category_default_for(Some(MeetingCategory::AllHands), &settings),
            Some(EnrollmentMode::Tracked)
        );
        assert_eq!(
            category_default_for(Some(MeetingCategory::Standup), &settings),
            Some(EnrollmentMode::ContextOnly)
        );
    }

    #[test]
    fn a_note_with_no_category_falls_through_the_chain_untouched() {
        // Every chat, and any meeting the classifier left unlabelled.
        assert_eq!(
            category_default_for(None, &CategorySettings::default()),
            None
        );
        assert_eq!(effective_mode(None, None), EnrollmentMode::Tracked);
    }

    #[test]
    fn the_chain_prefers_the_meetings_own_judgement() {
        assert_eq!(
            effective_mode(
                Some(EnrollmentMode::Tracked),
                Some(EnrollmentMode::ContextOnly)
            ),
            EnrollmentMode::Tracked
        );
        assert_eq!(
            effective_mode(
                Some(EnrollmentMode::ContextOnly),
                Some(EnrollmentMode::Tracked)
            ),
            EnrollmentMode::ContextOnly
        );
        assert_eq!(
            effective_mode(None, Some(EnrollmentMode::ContextOnly)),
            EnrollmentMode::ContextOnly
        );
    }

    #[test]
    fn category_provenance_round_trips_through_its_stored_spelling() {
        assert_eq!(EnrolledVia::Category.as_str(), "category");
        assert_eq!(
            EnrolledVia::parse("category").unwrap(),
            EnrolledVia::Category
        );
        assert_eq!(UntrackedVia::Category.as_str(), "category");
        assert_eq!(
            UntrackedVia::parse("category").unwrap(),
            UntrackedVia::Category
        );
    }

    #[test]
    fn open_in_memory_is_ready_to_use() {
        let ledger = Ledger::open_in_memory().unwrap();
        let count: i64 = ledger
            .conn
            .query_row("SELECT count(*) FROM ledger_entries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn open_creates_the_file_and_survives_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LEDGER_DB_FILE);
        {
            let ledger = Ledger::open(&path).unwrap();
            ledger
                .conn
                .execute("PRAGMA user_version", [])
                .ok()
                .or(Some(0));
        }
        assert!(path.is_file());
        // Reopening runs no migration and keeps the version.
        let ledger = Ledger::open(&path).unwrap();
        let version: i64 = ledger
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, migrations::CURRENT_VERSION);
    }

    #[test]
    fn a_busy_timeout_is_configured_so_two_writers_wait_rather_than_fail() {
        let dir = tempdir().unwrap();
        let ledger = Ledger::open(&dir.path().join(LEDGER_DB_FILE)).unwrap();
        let timeout: i64 = ledger
            .conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(timeout, i64::from(BUSY_TIMEOUT_MS));
    }

    #[test]
    fn a_second_handle_on_one_file_sees_the_first_handles_committed_write() {
        // The shape the MCP server introduces: its own connection writes the
        // judgement, and the app's long-lived worker connection must see it on
        // its next read rather than serving a cached view.
        let dir = tempdir().unwrap();
        let path = dir.path().join(LEDGER_DB_FILE);
        let mut writer = Ledger::open(&path).unwrap();
        let reader = Ledger::open(&path).unwrap();

        let outcome = writer
            .sync_note_items(&crate::ledger::NoteSync {
                note_id: "n_a1b2c3",
                project: "Briarwood",
                note_date_utc: "2026-08-21T10:00:00Z",
                items: &[crate::meeting::ActionItemFact {
                    id: "a_one".to_string(),
                    description: "ship the thing".to_string(),
                    owner: "Jane".to_string(),
                    due_date: None,
                    done: false,
                    firm: true,
                    extracted_date: None,
                }],
                link_hints: &[],
                note_override: None,
                category_default: None,
                identity: &OwnerIdentity::default(),
                now: "2026-08-21T10:00:00Z",
            })
            .unwrap();
        let entry_id = outcome.created.first().expect("an entry was created");

        let seen = reader.get_entry(entry_id).unwrap();
        assert!(seen.is_some(), "the second handle should see the commit");
    }

    #[test]
    fn minted_ids_are_well_formed_and_distinct() {
        let first = mint_id("le_").unwrap();
        let second = mint_id("le_").unwrap();
        assert_ne!(first, second);
        assert!(is_minted_id(&first, "le_"), "{first} should be well formed");
        assert_eq!(first.strip_prefix("le_").unwrap().len(), ID_RANDOM_LEN);
        assert!(!is_minted_id(&first, "ev_"));
        assert!(!is_minted_id("le_UPPER123456", "le_"));
        assert!(!is_minted_id("le_abc", "le_"), "too short");
    }

    #[test]
    fn direction_reads_the_grammar_conventions() {
        assert_eq!(Direction::from_owner("You"), Direction::Mine);
        assert_eq!(Direction::from_owner("you"), Direction::Mine);
        assert_eq!(
            Direction::from_owner(crate::distill::UNASSIGNED_OWNER),
            Direction::Unassigned
        );
        assert_eq!(Direction::from_owner("Priya"), Direction::Theirs);
        // "Yourself" is a named other, not the local user.
        assert_eq!(Direction::from_owner("Yourself"), Direction::Theirs);
    }

    #[test]
    fn an_identity_resolves_the_users_own_names_to_mine() {
        let identity = OwnerIdentity::new("Avery", &["Avery Kim".to_string()]);
        assert_eq!(Direction::resolve("Avery", &identity), Direction::Mine);
        assert_eq!(Direction::resolve("Avery Kim", &identity), Direction::Mine);
        // Case and stray whitespace fold; the same name typed either way is the
        // same person.
        assert_eq!(Direction::resolve("  avery  ", &identity), Direction::Mine);
        assert_eq!(Direction::resolve("AVERY KIM", &identity), Direction::Mine);
        // Interior whitespace runs collapse too.
        assert_eq!(
            Direction::resolve("Avery   Kim", &identity),
            Direction::Mine
        );
        // Someone else stays theirs, and so does a name that merely starts the
        // same way: no prefix matching.
        assert_eq!(Direction::resolve("Priya", &identity), Direction::Theirs);
        assert_eq!(
            Direction::resolve("Avery Chen", &identity),
            Direction::Theirs
        );
    }

    #[test]
    fn the_grammars_own_tokens_outrank_any_alias() {
        // A user who somehow configured these as names must not be able to
        // redefine what the prompt's own vocabulary means.
        let identity = OwnerIdentity::new("You", &[crate::distill::UNASSIGNED_OWNER.to_string()]);
        assert_eq!(Direction::resolve("You", &identity), Direction::Mine);
        assert_eq!(
            Direction::resolve(crate::distill::UNASSIGNED_OWNER, &identity),
            Direction::Unassigned,
            "an unattributed line never becomes the user's own"
        );
    }

    #[test]
    fn the_two_unicode_spellings_of_an_accented_name_resolve_alike() {
        // NFC first, so a name typed into Settings and a name the model wrote
        // fold together even when the bytes differ.
        let identity = OwnerIdentity::new("Zo\u{00e9}", &[]);
        assert_eq!(
            Direction::resolve("Zoe\u{0301}", &identity),
            Direction::Mine
        );
    }

    #[test]
    fn an_empty_identity_resolves_exactly_as_the_identity_less_form() {
        let empty = OwnerIdentity::default();
        assert!(empty.is_empty());
        for owner in ["You", "Priya", crate::distill::UNASSIGNED_OWNER, "Yourself"] {
            assert_eq!(
                Direction::resolve(owner, &empty),
                Direction::from_owner(owner),
                "{owner} should resolve alike"
            );
        }
        // Blank spellings never join the set, so they cannot match a blank owner.
        let blank = OwnerIdentity::new("   ", &["".to_string()]);
        assert!(blank.is_empty());
        assert!(!blank.is_me(""));
    }

    #[test]
    fn a_claim_only_learns_a_spelling_the_app_does_not_already_own() {
        assert_eq!(learnable_alias("Priya"), Some("Priya"));
        assert_eq!(learnable_alias("  Priya Raman  "), Some("Priya Raman"));
        // The grammar's tokens are resolved before any alias is consulted, and
        // "Them" would claim every future unnamed them-side commitment.
        for reserved in [
            "You",
            "you",
            "Them",
            "them",
            crate::distill::UNASSIGNED_OWNER,
            "  ",
        ] {
            assert_eq!(
                learnable_alias(reserved),
                None,
                "{reserved} is not learnable"
            );
        }
    }

    #[test]
    fn entry_state_round_trips_and_flags_terminal() {
        for state in [
            EntryState::Open,
            EntryState::NeedsReview,
            EntryState::Closed,
            EntryState::Superseded,
            EntryState::Waived,
            EntryState::Snoozed,
        ] {
            assert_eq!(EntryState::parse(state.as_str()).unwrap(), state);
        }
        assert!(EntryState::Closed.is_terminal());
        assert!(EntryState::Superseded.is_terminal());
        assert!(EntryState::Waived.is_terminal());
        assert!(!EntryState::Open.is_terminal());
        assert!(!EntryState::NeedsReview.is_terminal());
        assert!(!EntryState::Snoozed.is_terminal());
        assert!(EntryState::parse("done").is_err());
    }
}
