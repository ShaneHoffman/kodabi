//! Distilled notes as plain Markdown + YAML frontmatter — the source of truth
//! the whole knowledge base is built from (`docs/FRONTMATTER_SCHEMA.md`). The
//! SQLite index (Phase 2 watcher/rebuild) is derived from these files, never
//! the other way around, so the files stay Obsidian-compatible, git-diffable,
//! and free of lock-in.
//!
//! This module is the schema's first real consumer: it emits the seven
//! frontmatter keys in their locked canonical order — `id, type, project,
//! date, tags, source, confidence` — with the field-presence rules the schema
//! specifies, and round-trips (`struct → md → struct`). The frontmatter fields
//! mirror the MCP `NoteSummary` shape (`docs/MCP_TOOL_SURFACE.md`); the two
//! specs must stay in agreement.
//!
//! ## Representation choices the writer owns
//! - `project: Inbox` is a sentinel string (the MCP layer maps it to `null`);
//!   it also names a literal `<vault>/Inbox/` folder on disk.
//! - `tags` is omitted entirely when empty (the MCP layer reports `[]`), and
//!   emitted as an inline flow list (`tags: [a, b]`) so it matches the schema
//!   examples and reads natively in Obsidian.
//! - `confidence` is present exactly when a routing score backs `project` —
//!   including low-score notes that land in `Inbox`. It is omitted only for a
//!   hand-filed or imported note. [`Routing`] makes that rule unrepresentable
//!   when wrong: [`Routing::Routed`] always carries the score, [`Routing::Manual`]
//!   never does. A hand *re-route* correction (which records `1.0`) is simply a
//!   `Routed { confidence: 1.0 }`.

use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

use crate::naming;

/// Base36 alphabet used for the random part of a [`NoteId`] (mirrors
/// `device.rs`).
const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
/// Number of random base36 characters after the `n_` prefix. The schema regex
/// (`^n_[0-9a-z]{6,}$`) permits any length ≥ 6; 8 (like `DeviceId`) is chosen
/// deliberately: an `id` collision is unrecoverable (the writer's collision
/// safety only disambiguates *filenames*), and import/merge can pool several
/// devices' notes into one vault. 36^8 ≈ 2.8e12 keeps collisions negligible.
const NOTE_ID_RANDOM_LEN: usize = 8;
/// Minimum random-part length the schema regex accepts (`{6,}`).
const NOTE_ID_MIN_RANDOM_LEN: usize = 6;

/// The sentinel `project` value for a note routing could not confidently file.
/// Not a real project — it maps to a literal `<vault>/Inbox/` folder and, at
/// the MCP layer, to `project: null`.
pub const INBOX: &str = "Inbox";

/// Knowledge-base root directory names owned by other subsystems — the
/// transcription pipeline's raw-session-artifact tree
/// (`docs/FRONTMATTER_SCHEMA.md` places `raw/…` relative to the vault root),
/// its `sessions/` tree, and WebView2's data dir (the KB root is currently the
/// Tauri `app_data_dir`, which they share). A project may not claim one as its
/// first segment — vault enumeration and project discovery (`routing`) skip
/// these dirs, so a note filed there would be invisible to the UI — but deeper
/// segments stay legal (`Data/raw` is a real project). Reserved in any casing
/// — the filesystem is case-insensitive on Windows.
pub(crate) const RESERVED_ROOT_DIRS: &[&str] = &["sessions", "raw", "EBWebView"];

/// Per-process counter that, combined with the process id, gives each in-flight
/// write a unique scratch filename so concurrent writes can't clobber each
/// other's temp file (mirrors `raw_session.rs` / `glossary.rs`).
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors produced while emitting, parsing, or writing a note.
#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error("note I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("note frontmatter YAML error: {source}")]
    Yaml {
        #[source]
        source: serde_yaml_ng::Error,
    },
    #[error("note is missing its `---` frontmatter fences")]
    MissingFrontmatter,
    #[error("invalid note {field}: {detail}")]
    InvalidField { field: &'static str, detail: String },
    #[error(
        "note id {id} is claimed by more than one file in the same folder ({paths:?}); \
         resolve the duplicate on disk before opening or editing it"
    )]
    DuplicateNoteId { id: String, paths: Vec<PathBuf> },
    #[error("target project {project:?} does not exist; pass create_project to create it")]
    MissingProject { project: String },
}

/// `Result` specialised to [`NoteError`].
pub type Result<T> = std::result::Result<T, NoteError>;

fn invalid(field: &'static str, detail: impl Into<String>) -> NoteError {
    NoteError::InvalidField {
        field,
        detail: detail.into(),
    }
}

// ---------------------------------------------------------------------------
// NoteId
// ---------------------------------------------------------------------------

/// A note's stable identity and MCP write handle: `n_` + base36, matching
/// `^n_[0-9a-z]{6,}$`. Generated once at creation and **never** rewritten on
/// move or re-route — the file path is informational and changes on move; the
/// `id` never does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NoteId(String);

impl NoteId {
    /// Borrows the underlying `n_…` string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validates and wraps an existing string as a `NoteId`.
    pub fn parse(s: &str) -> std::result::Result<Self, InvalidNoteId> {
        let rest = s.strip_prefix("n_").ok_or(InvalidNoteId)?;
        if rest.len() >= NOTE_ID_MIN_RANDOM_LEN && rest.bytes().all(|b| ALPHABET.contains(&b)) {
            Ok(Self(s.to_string()))
        } else {
            Err(InvalidNoteId)
        }
    }

    /// Generates a fresh random note id from OS entropy.
    ///
    /// Uses rejection sampling so every base36 symbol is equally likely — a
    /// plain `byte % 36` would over-weight `0`–`3` (mirrors `DeviceId::generate`).
    pub fn generate() -> io::Result<Self> {
        // Largest multiple of the alphabet size that fits in a byte (7·36 = 252);
        // bytes at or above it are discarded to keep the distribution uniform.
        const REJECT_THRESHOLD: u8 = (256 / ALPHABET.len() * ALPHABET.len()) as u8;

        let mut random = String::with_capacity(NOTE_ID_RANDOM_LEN);
        let mut buf = [0u8; NOTE_ID_RANDOM_LEN];
        while random.len() < NOTE_ID_RANDOM_LEN {
            getrandom::getrandom(&mut buf)
                .map_err(|err| io::Error::other(format!("OS RNG unavailable: {err}")))?;
            for &byte in &buf {
                if byte < REJECT_THRESHOLD {
                    random.push(ALPHABET[(byte % ALPHABET.len() as u8) as usize] as char);
                    if random.len() == NOTE_ID_RANDOM_LEN {
                        break;
                    }
                }
            }
        }
        Ok(Self(format!("n_{random}")))
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A string failed to parse as a valid [`NoteId`].
#[derive(Debug)]
pub struct InvalidNoteId;

impl fmt::Display for InvalidNoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("note id must match ^n_[0-9a-z]{6,}$")
    }
}

impl std::error::Error for InvalidNoteId {}

// ---------------------------------------------------------------------------
// NoteType
// ---------------------------------------------------------------------------

/// The closed set of note kinds (`docs/FRONTMATTER_SCHEMA.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteType {
    Meeting,
    Note,
    Chat,
}

impl NoteType {
    /// The frontmatter spelling of this type.
    pub fn as_str(&self) -> &'static str {
        match self {
            NoteType::Meeting => "meeting",
            NoteType::Note => "note",
            NoteType::Chat => "chat",
        }
    }

    /// Parses the frontmatter spelling; rejects anything outside the closed set
    /// (including wrong case like `Meeting`).
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "meeting" => Ok(NoteType::Meeting),
            "note" => Ok(NoteType::Note),
            "chat" => Ok(NoteType::Chat),
            other => Err(invalid(
                "type",
                format!("type {other:?} must be one of meeting | note | chat"),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Tag
// ---------------------------------------------------------------------------

/// A single note tag: lowercase kebab-case, no leading `#`
/// (`^[a-z0-9]+(?:-[a-z0-9]+)*$`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag(String);

impl Tag {
    /// Validates and wraps a tag string.
    pub fn parse(s: &str) -> Result<Self> {
        if is_kebab_case(s) {
            Ok(Self(s.to_string()))
        } else {
            Err(invalid(
                "tags",
                format!("tag {s:?} must be lowercase kebab-case with no leading '#'"),
            ))
        }
    }

    /// Parses user input leniently: trims surrounding whitespace and folds
    /// ASCII uppercase before validating. Frontmatter parsing stays strict
    /// ([`Tag::parse`]) — files on disk must already be canonical. This is the
    /// single home of the input-normalization policy, so every entry point
    /// (UI form, MCP tool, distill pipeline) folds identically.
    pub fn parse_normalized(s: &str) -> Result<Self> {
        Self::parse(&s.trim().to_ascii_lowercase())
    }

    /// Borrows the tag text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// True for a non-empty string of `[a-z0-9]` runs joined by single `-`, with no
/// leading, trailing, or doubled `-`.
fn is_kebab_case(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

/// How a note came to exist: either a capture keyword or a repo-relative path
/// to the raw session artifact it was distilled from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Keyword(SourceKeyword),
    RawArtifact(String),
}

/// The closed set of capture keywords a note without a raw artifact falls back
/// to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKeyword {
    Transcript,
    QuickCapture,
    Chat,
    Import,
    Manual,
}

impl SourceKeyword {
    /// The frontmatter spelling of this keyword.
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKeyword::Transcript => "transcript",
            SourceKeyword::QuickCapture => "quick-capture",
            SourceKeyword::Chat => "chat",
            SourceKeyword::Import => "import",
            SourceKeyword::Manual => "manual",
        }
    }

    fn from_keyword(s: &str) -> Option<Self> {
        match s {
            "transcript" => Some(SourceKeyword::Transcript),
            "quick-capture" => Some(SourceKeyword::QuickCapture),
            "chat" => Some(SourceKeyword::Chat),
            "import" => Some(SourceKeyword::Import),
            "manual" => Some(SourceKeyword::Manual),
            _ => None,
        }
    }
}

impl Source {
    /// Parses a `source` value. A value exactly equal to one of the five
    /// keywords is that keyword; anything else is treated as a repo-relative
    /// raw-artifact path (which must be non-empty, single-line, and not
    /// absolute).
    pub fn parse(s: &str) -> Result<Self> {
        if let Some(keyword) = SourceKeyword::from_keyword(s) {
            return Ok(Source::Keyword(keyword));
        }
        validate_raw_artifact(s)?;
        Ok(Source::RawArtifact(s.to_string()))
    }

    /// The exact string this source serializes to in frontmatter.
    pub fn as_yaml(&self) -> &str {
        match self {
            Source::Keyword(keyword) => keyword.as_str(),
            Source::RawArtifact(path) => path,
        }
    }
}

fn validate_raw_artifact(s: &str) -> Result<()> {
    if s.is_empty() || s.contains('\n') || s.contains('\r') {
        return Err(invalid(
            "source",
            "source must be a non-empty, single-line value",
        ));
    }
    // Repo-relative with '/' separators only: a backslash (Windows-style or a
    // UNC `\\server\…` prefix) breaks the cross-platform, forward-slash path
    // contract — mirrors how `validate_project` rejects `\`.
    if s.contains('\\') {
        return Err(invalid(
            "source",
            format!("raw-artifact path {s:?} must use '/' as the separator, not '\\'"),
        ));
    }
    // Repo-relative: reject POSIX absolute paths and Windows drive letters.
    let looks_absolute = s.starts_with('/') || s.as_bytes().get(1) == Some(&b':');
    if looks_absolute {
        return Err(invalid(
            "source",
            format!("raw-artifact path {s:?} must be repo-relative, not absolute"),
        ));
    }
    // No `.`/`..` traversal segments: the path is joined under the KB root, so a
    // `..` segment would escape the vault.
    if s.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(invalid(
            "source",
            format!("raw-artifact path {s:?} must not contain '.' or '..' path segments"),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

/// A note's `project` filing together with its (optional) routing score.
///
/// This is where the schema's subtlest rule lives: `confidence` is present
/// exactly when a routing score backs `project`. `Routed` always carries the
/// score (its `project` may be the [`INBOX`] sentinel); `Manual` — a hand-filed
/// or imported note — never does.
#[derive(Debug, Clone, PartialEq)]
pub enum Routing {
    /// Confidence-split routing filed the note (auto-routed, re-routed, or an
    /// Inbox landing). `project` may be [`INBOX`].
    Routed { project: String, confidence: f64 },
    /// A human filed the note directly, or it was imported — no routing score.
    /// `project` may not be [`INBOX`] (Inbox is always routing-scored).
    Manual { project: String },
}

impl Routing {
    /// The `project` value, regardless of how the note was filed.
    pub fn project(&self) -> &str {
        match self {
            Routing::Routed { project, .. } => project,
            Routing::Manual { project } => project,
        }
    }

    /// The routing score, if one backs `project`.
    pub fn confidence(&self) -> Option<f64> {
        match self {
            Routing::Routed { confidence, .. } => Some(*confidence),
            Routing::Manual { .. } => None,
        }
    }

    /// Builds routing from the flat `(project, confidence)` pair the frontmatter
    /// and IPC layers both carry, keeping the "confidence ⇔ routing" rule in one
    /// place: a present score means the note is routed (auto-filed, re-routed,
    /// or an Inbox landing); an absent score means it was hand-filed — which the
    /// [`INBOX`] sentinel (always routing-scored) may not be.
    pub fn from_project_and_confidence(project: String, confidence: Option<f64>) -> Result<Self> {
        match confidence {
            Some(confidence) => Ok(Routing::Routed {
                project,
                confidence,
            }),
            None => {
                if project == INBOX {
                    return Err(invalid(
                        "confidence",
                        "an Inbox note is routing-scored; its confidence must be present",
                    ));
                }
                Ok(Routing::Manual { project })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Note
// ---------------------------------------------------------------------------

/// The editable subset of a note — what the edit flow may replace. Everything
/// else (`id`, `source`, routing) is preserved verbatim by
/// [`Note::with_edits`]; moving a note between projects is the re-route flow,
/// not an edit.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteEdit {
    pub note_type: NoteType,
    pub date: String,
    pub tags: Vec<Tag>,
    pub body: String,
}

/// A single note: its seven frontmatter fields plus the Markdown body.
///
/// Construct through [`Note::new`], which validates every field and normalizes
/// the body; the public fields are exposed for read access and for building in
/// tests, but bypassing `new` skips validation.
#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: NoteId,
    pub note_type: NoteType,
    pub routing: Routing,
    /// ISO 8601, stored **verbatim** — a full RFC 3339 timestamp with offset
    /// when a time is known, or a date-only `YYYY-MM-DD` otherwise. Never
    /// reformatted.
    pub date: String,
    pub tags: Vec<Tag>,
    pub source: Source,
    /// The Markdown body, stored trimmed of surrounding blank lines (its
    /// canonical form).
    pub body: String,
}

impl Note {
    /// Builds a validated note, normalizing the body to its trimmed canonical
    /// form. Returns [`NoteError::InvalidField`] if any field breaks the schema.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: NoteId,
        note_type: NoteType,
        routing: Routing,
        date: impl Into<String>,
        tags: Vec<Tag>,
        source: Source,
        body: impl Into<String>,
    ) -> Result<Self> {
        let note = Note {
            id,
            note_type,
            routing,
            date: date.into(),
            tags,
            source,
            body: body.into().trim().to_string(),
        };
        note.validate()?;
        Ok(note)
    }

    /// Applies an edit, preserving `id`, `source`, and routing (project +
    /// confidence) verbatim and replacing only the editable fields; the result
    /// is re-validated. This is the edit path's preservation contract — it
    /// lives here so every shell (Tauri command, MCP `edit_note`) shares one
    /// implementation instead of re-merging by hand.
    pub fn with_edits(self, edit: NoteEdit) -> Result<Self> {
        Note::new(
            self.id,
            edit.note_type,
            self.routing,
            edit.date,
            edit.tags,
            self.source,
            edit.body,
        )
    }

    /// Checks every field against the schema. The type of each field already
    /// guarantees its own shape (a [`Tag`] is always kebab-case, a [`NoteType`]
    /// is always in the closed set); this covers the cross-field and
    /// still-`String` rules: project slug, date shape, confidence range, the
    /// routing/Inbox invariant, and a raw-artifact source built directly.
    pub fn validate(&self) -> Result<()> {
        validate_project(self.routing.project())?;
        if let Routing::Manual { project } = &self.routing {
            if project == INBOX {
                return Err(invalid(
                    "project",
                    "the Inbox sentinel is always routing-scored; a hand-filed \
                     (Manual) note cannot be in Inbox",
                ));
            }
        }
        validate_date(&self.date)?;
        if let Some(confidence) = self.routing.confidence() {
            if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
                return Err(invalid(
                    "confidence",
                    format!("confidence {confidence} must be within 0.0..=1.0"),
                ));
            }
        }
        if let Source::RawArtifact(path) = &self.source {
            validate_raw_artifact(path)?;
        }
        Ok(())
    }

    /// Serializes the note to Markdown + YAML frontmatter in canonical key
    /// order. Total: assumes a validated note (build via [`Note::new`]).
    pub fn to_markdown(&self) -> String {
        let mut fm = String::from("---\n");
        // `id` (n_… base36) and `type` (closed enum) are always plain-safe, so
        // they are emitted raw. Every other scalar goes through the YAML
        // serializer so a value needing quotes (a project literally named
        // `true`, or one containing `: `) round-trips as a string.
        let _ = writeln!(fm, "id: {}", self.id);
        let _ = writeln!(fm, "type: {}", self.note_type.as_str());
        let _ = writeln!(fm, "project: {}", emit_scalar_str(self.routing.project()));
        // `date` is emitted verbatim (validation guarantees a plain-safe shape).
        let _ = writeln!(fm, "date: {}", self.date);
        if !self.tags.is_empty() {
            let joined = self
                .tags
                .iter()
                .map(|tag| emit_scalar_str(tag.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(fm, "tags: [{joined}]");
        }
        let _ = writeln!(fm, "source: {}", emit_scalar_str(self.source.as_yaml()));
        if let Some(confidence) = self.routing.confidence() {
            let _ = writeln!(fm, "confidence: {}", emit_scalar_f64(confidence));
        }
        fm.push_str("---\n");

        if self.body.is_empty() {
            fm
        } else {
            format!("{fm}\n{}\n", self.body)
        }
    }

    /// Parses Markdown + YAML frontmatter back into a validated note. The
    /// inverse of [`Note::to_markdown`] for any note it produced.
    pub fn from_markdown(input: &str) -> Result<Self> {
        let input = input.strip_prefix('\u{feff}').unwrap_or(input);
        let (frontmatter, body) = split_frontmatter(input)?;

        let raw: RawFrontmatter =
            serde_yaml_ng::from_str(frontmatter).map_err(|source| NoteError::Yaml { source })?;

        let id = NoteId::parse(&raw.id).map_err(|_| {
            invalid(
                "id",
                format!("id {:?} must match ^n_[0-9a-z]{{6,}}$", raw.id),
            )
        })?;
        let note_type = NoteType::parse(&raw.note_type)?;
        let tags = raw
            .tags
            .unwrap_or_default()
            .iter()
            .map(|t| Tag::parse(t))
            .collect::<Result<Vec<_>>>()?;
        let source = Source::parse(&raw.source)?;
        let routing = Routing::from_project_and_confidence(raw.project, raw.confidence)?;

        Note::new(id, note_type, routing, raw.date, tags, source, body)
    }
}

/// The flat intermediate the frontmatter YAML block deserializes into before it
/// is mapped and validated into a [`Note`]. Unknown keys are tolerated (and
/// dropped on rewrite); absent `tags`/`confidence` become `None`.
#[derive(Deserialize)]
struct RawFrontmatter {
    id: String,
    #[serde(rename = "type")]
    note_type: String,
    project: String,
    date: String,
    #[serde(default)]
    tags: Option<Vec<String>>,
    source: String,
    #[serde(default)]
    confidence: Option<f64>,
}

/// Serializes a single string scalar to its minimal correct YAML form (quoting
/// only when the value would otherwise re-resolve as a non-string). Infallible
/// for an in-memory string.
fn emit_scalar_str(s: &str) -> String {
    serde_yaml_ng::to_string(s)
        .expect("serializing a string scalar to YAML is infallible")
        .trim_end()
        .to_string()
}

/// Serializes a float scalar (`ryu` always keeps a decimal point, so `1.0`
/// stays `1.0`, never `1`).
fn emit_scalar_f64(value: f64) -> String {
    serde_yaml_ng::to_string(&value)
        .expect("serializing a float scalar to YAML is infallible")
        .trim_end()
        .to_string()
}

/// Splits `input` into (frontmatter YAML block, body), operating on byte
/// offsets so body bytes survive exactly.
///
/// The first line must be exactly `---`; the block runs to the **first**
/// subsequent line equal to `---`, so a `---` horizontal rule inside the body
/// is never mistaken for the closing fence. A missing opening or closing fence
/// is a [`NoteError::MissingFrontmatter`].
fn split_frontmatter(input: &str) -> Result<(&str, &str)> {
    let (first_line, after_first) = match input.find('\n') {
        Some(idx) => (&input[..idx], &input[idx + 1..]),
        None => (input, ""),
    };
    if first_line.trim_end_matches('\r') != "---" {
        return Err(NoteError::MissingFrontmatter);
    }

    let mut offset = 0usize;
    loop {
        let remaining = &after_first[offset..];
        let (line, line_len_incl_nl, is_last) = match remaining.find('\n') {
            Some(idx) => (&remaining[..idx], idx + 1, false),
            None => (remaining, remaining.len(), true),
        };
        if line.trim_end_matches('\r') == "---" {
            let frontmatter = &after_first[..offset];
            let body = after_first.get(offset + line_len_incl_nl..).unwrap_or("");
            return Ok((frontmatter, body));
        }
        if is_last {
            return Err(NoteError::MissingFrontmatter);
        }
        offset += line_len_incl_nl;
    }
}

// ---------------------------------------------------------------------------
// Field validation
// ---------------------------------------------------------------------------

/// Windows reserved device names; a project folder segment may not be one of
/// these (with or without an extension).
const RESERVED_SEGMENTS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Maximum `project` length, mirroring the MCP `ProjectSlug` `maxLength`
/// (`docs/MCP_TOOL_SURFACE.md`) so the writer and the tool surface agree.
const MAX_PROJECT_LEN: usize = 300;

pub(crate) fn validate_project(project: &str) -> Result<()> {
    if project == INBOX {
        return Ok(());
    }
    if project.is_empty() || project.contains('\n') || project.contains('\r') {
        return Err(invalid(
            "project",
            "project must be a non-empty, single-line value",
        ));
    }
    if project.len() > MAX_PROJECT_LEN {
        return Err(invalid(
            "project",
            format!(
                "project must be at most {MAX_PROJECT_LEN} characters (MCP ProjectSlug maxLength)"
            ),
        ));
    }
    // The `Inbox` sentinel owns `<vault>/Inbox/`. A real project whose first
    // segment case-folds to `Inbox` would map into that same folder on a
    // case-insensitive filesystem, mixing routed notes into the unfiled-Inbox
    // tree, so `Inbox` is a reserved folder name. The exact-string sentinel is
    // handled by the early return above; this catches every other casing
    // (`inbox`, `INBOX`) and a nested `Inbox/…` landing.
    let first_segment = project.split('/').next().unwrap_or(project);
    if first_segment.eq_ignore_ascii_case(INBOX) {
        return Err(invalid(
            "project",
            format!("project segment {first_segment:?} is reserved: a real project may not be named `Inbox`"),
        ));
    }
    // Same hazard as `Inbox` for the other reserved root dirs (`raw/` holds
    // raw session artifacts, not notes): writing there succeeds, but
    // enumeration skips those trees, stranding the note. Only the first
    // segment is reserved — `Data/raw` stays a real project.
    if RESERVED_ROOT_DIRS
        .iter()
        .any(|reserved| first_segment.eq_ignore_ascii_case(reserved))
    {
        return Err(invalid(
            "project",
            format!("project segment {first_segment:?} is reserved for internal app data"),
        ));
    }
    if project.contains('\\') {
        return Err(invalid(
            "project",
            format!("project {project:?} must use '/' as the segment separator, not '\\'"),
        ));
    }
    if project.split('/').any(str::is_empty) {
        return Err(invalid(
            "project",
            format!("project {project:?} has an empty path segment (no leading, trailing, or doubled '/')"),
        ));
    }
    for segment in project.split('/') {
        validate_folder_segment(segment)?;
    }
    Ok(())
}

/// A project path segment becomes a real folder, so it must be a legal Windows
/// folder name beyond just matching the slug regex.
pub(crate) fn validate_folder_segment(segment: &str) -> Result<()> {
    const FORBIDDEN: &[char] = &[':', '*', '?', '"', '<', '>', '|'];
    if segment
        .chars()
        .any(|c| FORBIDDEN.contains(&c) || c.is_control())
    {
        return Err(invalid(
            "project",
            format!(
                "project segment {segment:?} contains a character not allowed in a folder name"
            ),
        ));
    }
    if segment.ends_with('.') || segment.ends_with(' ') {
        return Err(invalid(
            "project",
            format!("project segment {segment:?} may not end with a dot or space"),
        ));
    }
    // Dot- and underscore-prefixed folders are infra (`.obsidian`, `_assets`,
    // `_glossary.yml`'s home) and project discovery skips them at every depth
    // (`routing`), so a project named like one would be writable yet invisible
    // to routing forever — its glossary never loaded, its meetings drifting to
    // Inbox with no error. Reject it here, at the choke point every writer
    // shares, so the two definitions of "project" stay equal.
    if segment.starts_with('.') || segment.starts_with('_') {
        return Err(invalid(
            "project",
            format!("project segment {segment:?} may not start with '.' or '_' (reserved for infra folders)"),
        ));
    }
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .to_ascii_lowercase();
    if RESERVED_SEGMENTS.contains(&stem.as_str()) {
        return Err(invalid(
            "project",
            format!("project segment {segment:?} is a reserved Windows device name"),
        ));
    }
    Ok(())
}

fn validate_date(date: &str) -> Result<()> {
    if is_date_only(date) || chrono::DateTime::parse_from_rfc3339(date).is_ok() {
        return Ok(());
    }
    Err(invalid(
        "date",
        format!(
            "date {date:?} must be YYYY-MM-DD or an RFC 3339 timestamp with offset \
             (e.g. 2026-07-09T14:00:00-07:00)"
        ),
    ))
}

/// True for a strict `YYYY-MM-DD` that is also a real calendar date.
fn is_date_only(date: &str) -> bool {
    let b = date.as_bytes();
    date.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b[0..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
        && chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok()
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Writes `note` into its per-project folder under `vault_root` as
/// `{slug(title)}.md`, and returns the path written.
///
/// The project maps to a folder path: a hierarchical `Growth/Q3` becomes nested
/// directories, and the [`INBOX`] sentinel becomes `<vault>/Inbox/`; missing
/// parents are created. When `title` is absent or slugifies to nothing, the
/// filename falls back to the note `id` (always filesystem-safe).
///
/// Never overwrites an existing file: a taken name is disambiguated with an
/// increasing numeric suffix, and the claim is atomic — the destination is
/// created by hard-linking a fully written scratch file, which fails rather than
/// clobbering when the name exists, so concurrent writers can't collide (mirrors
/// `raw_session::write_raw_session`).
pub fn write_note(vault_root: &Path, note: &Note, title: Option<&str>) -> Result<PathBuf> {
    note.validate()?;

    let dir = project_dir(vault_root, note.routing.project());
    fs::create_dir_all(&dir).map_err(|source| NoteError::Io {
        path: dir.clone(),
        source,
    })?;

    let markdown = note.to_markdown();
    let tmp_path = stage_scratch(&dir, &markdown)?;
    let result = link_into_free_path(&tmp_path, &dir, note, title);
    // The linked destination keeps the content alive; the scratch name is
    // redundant on both success and failure.
    let _ = fs::remove_file(&tmp_path);
    result
}

/// Rewrites an existing note file in place, atomically. The complement of
/// [`write_note`] for the edit path: the filename never changes on edit.
/// Build the note via [`Note::with_edits`] (or the whole-flow
/// `vault::save_note_edit`) so `id`, `source`, and routing are preserved from
/// the parsed original.
///
/// The scratch file is staged in the same directory, so the `fs::rename` is a
/// same-volume atomic replace (`MoveFileExW` + `MOVEFILE_REPLACE_EXISTING` on
/// Windows) — a concurrent reader never observes a partial file. Errors with
/// `NotFound` when `path` is no longer a file (the note was deleted or moved
/// externally) rather than re-creating it; the check-then-rename window is
/// accepted for a single-user desktop vault.
pub fn save_note_at(path: &Path, note: &Note) -> Result<()> {
    note.validate()?;

    if !path.is_file() {
        return Err(NoteError::Io {
            path: path.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::NotFound,
                "note file no longer exists; it may have been deleted or moved",
            ),
        });
    }
    let dir = path.parent().ok_or_else(|| NoteError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(
            io::ErrorKind::InvalidInput,
            "note path has no parent directory",
        ),
    })?;

    let tmp_path = stage_scratch(dir, &note.to_markdown())?;
    if let Err(source) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(NoteError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}

/// Moves a note file into `target_dir`, writing `note`'s new content and
/// removing the original — the primitive under the re-route flow
/// (`vault::file_note_to_project`). The complement of [`write_note`] (which
/// slugs a fresh filename from the title) and [`save_note_at`] (which never
/// changes the folder): here the note's project has changed, so the file must
/// move to a new folder while keeping its **existing filename stem** (the path
/// is informational, and re-slugging would rename an id-fallback or a
/// hand-renamed file). `target_dir` is created if missing.
///
/// Atomic and collision-safe like `write_note`: a fully written scratch file is
/// hard-linked into the first free `{stem}.md` (numeric suffix on clash), so a
/// concurrent writer can't clobber. The original is removed only after the new
/// link exists; if that removal fails the new link is rolled back, so the vault
/// never ends with two files claiming one id. Returns the new path.
pub(crate) fn relocate_note_file(
    note: &Note,
    old_path: &Path,
    target_dir: &Path,
) -> Result<PathBuf> {
    note.validate()?;

    // Preserve the original filename stem; fall back to the id if the path has
    // none (unreachable for a real note file, but keeps this total).
    let stem = old_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| note.id.as_str())
        .to_string();

    fs::create_dir_all(target_dir).map_err(|source| NoteError::Io {
        path: target_dir.to_path_buf(),
        source,
    })?;

    let tmp_path = stage_scratch(target_dir, &note.to_markdown())?;
    let new_path = link_into_free_stem(&tmp_path, target_dir, &stem);
    // The linked destination keeps the content alive; the scratch name is
    // redundant on both success and failure.
    let _ = fs::remove_file(&tmp_path);
    let new_path = new_path?;

    // Remove the original only now that the move target exists. On failure roll
    // the new link back so exactly one file ever claims the id.
    if let Err(source) = fs::remove_file(old_path) {
        let _ = fs::remove_file(&new_path);
        return Err(NoteError::Io {
            path: old_path.to_path_buf(),
            source,
        });
    }
    Ok(new_path)
}

/// Resolves the on-disk folder for a project, splitting a hierarchical slug into
/// nested segments (so separators are correct on every OS). `validate_project`
/// guarantees no empty segments and no backslash, so this can't escape or
/// mis-nest.
pub(crate) fn project_dir(vault_root: &Path, project: &str) -> PathBuf {
    let mut dir = vault_root.to_path_buf();
    for segment in project.split('/') {
        dir.push(segment);
    }
    dir
}

/// Writes `contents` to a fresh, process-unique scratch file in `dir`. On a
/// write error the partial scratch file is removed, so a failed write leaves
/// nothing behind.
fn stage_scratch(dir: &Path, contents: &str) -> Result<PathBuf> {
    let tmp_path = dir.join(format!(
        ".note.{}.{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if let Err(source) = fs::write(&tmp_path, contents) {
        let _ = fs::remove_file(&tmp_path);
        return Err(NoteError::Io {
            path: tmp_path,
            source,
        });
    }
    Ok(tmp_path)
}

/// Hard-links the staged scratch file into the first free `{stem}.md` name under
/// `dir`, and returns that path. The disambiguator comes from
/// [`naming::numbered_slug`], whose number survives the filename length cap, so
/// distinct attempts always produce distinct names and the loop terminates.
fn link_into_free_path(
    tmp_path: &Path,
    dir: &Path,
    note: &Note,
    title: Option<&str>,
) -> Result<PathBuf> {
    let title_slug = title.map(naming::slug).filter(|s| !s.is_empty());
    let base = title_slug
        .clone()
        .unwrap_or_else(|| note.id.as_str().to_string());

    let mut attempt: Option<u32> = None;
    loop {
        let stem = match attempt {
            None => base.clone(),
            Some(n) => match title_slug {
                Some(_) => naming::numbered_slug(title, n),
                None => format!("{base}-{n}"),
            },
        };
        let candidate = dir.join(format!("{stem}.md"));
        match fs::hard_link(tmp_path, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                attempt = Some(attempt.map_or(2, |n| n + 1));
            }
            Err(source) => {
                return Err(NoteError::Io {
                    path: candidate,
                    source,
                });
            }
        }
    }
}

/// Hard-links the staged scratch file into the first free name derived from a
/// fixed `stem` under `dir` (`{stem}.md`, then `{stem}-2.md`, `{stem}-3.md`…),
/// and returns that path. Unlike [`link_into_free_path`], the stem is given (the
/// original filename on a move) rather than slugged from a title; the numeric
/// suffix guarantees distinct attempts, so the loop terminates.
fn link_into_free_stem(tmp_path: &Path, dir: &Path, stem: &str) -> Result<PathBuf> {
    let mut attempt: Option<u32> = None;
    loop {
        let name = match attempt {
            None => format!("{stem}.md"),
            Some(n) => format!("{stem}-{n}.md"),
        };
        let candidate = dir.join(name);
        match fs::hard_link(tmp_path, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                attempt = Some(attempt.map_or(2, |n| n + 1));
            }
            Err(source) => {
                return Err(NoteError::Io {
                    path: candidate,
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // --- helpers ----------------------------------------------------------

    fn id() -> NoteId {
        NoteId::parse("n_a1b2c3").unwrap()
    }

    fn tags(items: &[&str]) -> Vec<Tag> {
        items.iter().map(|t| Tag::parse(t).unwrap()).collect()
    }

    fn source(s: &str) -> Source {
        Source::parse(s).unwrap()
    }

    /// The `meeting` example from `docs/FRONTMATTER_SCHEMA.md`, verbatim.
    const MEETING_MD: &str = "\
---
id: n_a1b2c3
type: meeting
project: Briarwood Golf
date: 2026-07-09T14:00:00-07:00
tags: [budgeting, phase-2]
source: raw/20260709T210000000Z-k4m2xp7q-briarwood-golf-sync.jsonl
confidence: 0.94
---

# Summary

Reviewed Q3 budget allocation for the course renovation and agreed on the irrigation
contractor shortlist.

## Decisions

- Approved the revised irrigation budget of $42,000.
- Selected GreenFlow Systems as the lead contractor for bidding.

## Action items

- [ ] Shane to send the signed budget memo to finance by 2026-07-11.
- [ ] Priya to request formal bids from GreenFlow and two alternates.
";

    fn meeting_note() -> Note {
        Note::new(
            id(),
            NoteType::Meeting,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 0.94,
            },
            "2026-07-09T14:00:00-07:00",
            tags(&["budgeting", "phase-2"]),
            source("raw/20260709T210000000Z-k4m2xp7q-briarwood-golf-sync.jsonl"),
            "# Summary\n\nReviewed Q3 budget allocation for the course renovation and agreed on the irrigation\ncontractor shortlist.\n\n## Decisions\n\n- Approved the revised irrigation budget of $42,000.\n- Selected GreenFlow Systems as the lead contractor for bidding.\n\n## Action items\n\n- [ ] Shane to send the signed budget memo to finance by 2026-07-11.\n- [ ] Priya to request formal bids from GreenFlow and two alternates.",
        )
        .unwrap()
    }

    fn note_inbox() -> Note {
        Note::new(
            NoteId::parse("n_d4e5f6").unwrap(),
            NoteType::Note,
            Routing::Routed {
                project: INBOX.to_string(),
                confidence: 0.38,
            },
            "2026-07-10",
            tags(&["idea"]),
            source("quick-capture"),
            "# Note\n\nMaybe worth checking if the course's irrigation contractor also handles the clubhouse\ndrainage — ask Priya next sync.",
        )
        .unwrap()
    }

    fn chat_manual() -> Note {
        Note::new(
            NoteId::parse("n_g7h8i9").unwrap(),
            NoteType::Chat,
            Routing::Manual {
                project: "Briarwood Golf".to_string(),
            },
            "2026-07-10T09:15:00-07:00",
            tags(&["research"]),
            source("raw/20260710T161500000Z-k4m2xp7q-irrigation-contractor-comparison.jsonl"),
            "# Chat: irrigation contractor comparison\n\nAsked Claude Code to compare GreenFlow Systems against the two alternate bidders using\nprior meeting notes filed under this project; kept the comparison table as a reference\nnote.",
        )
        .unwrap()
    }

    // --- exact bytes ------------------------------------------------------

    #[test]
    fn meeting_emits_exact_canonical_bytes() {
        assert_eq!(meeting_note().to_markdown(), MEETING_MD);
    }

    #[test]
    fn note_inbox_emits_confidence_even_in_inbox() {
        let md = note_inbox().to_markdown();
        assert!(md.contains("project: Inbox"));
        assert!(md.contains("confidence: 0.38"));
        // canonical order: project before date before tags before source.
        let project_at = md.find("project:").unwrap();
        let source_at = md.find("source:").unwrap();
        let confidence_at = md.find("confidence:").unwrap();
        assert!(project_at < source_at && source_at < confidence_at);
    }

    #[test]
    fn chat_manual_omits_confidence_key() {
        let md = chat_manual().to_markdown();
        assert!(
            !md.contains("confidence:"),
            "manual note must omit confidence"
        );
        assert!(md.contains("type: chat"));
    }

    // --- round trips ------------------------------------------------------

    #[test]
    fn round_trips_meeting() {
        let note = meeting_note();
        assert_eq!(Note::from_markdown(&note.to_markdown()).unwrap(), note);
    }

    #[test]
    fn round_trips_note_inbox() {
        let note = note_inbox();
        assert_eq!(Note::from_markdown(&note.to_markdown()).unwrap(), note);
    }

    #[test]
    fn round_trips_chat_manual() {
        let note = chat_manual();
        assert_eq!(Note::from_markdown(&note.to_markdown()).unwrap(), note);
    }

    #[test]
    fn parses_the_verbatim_meeting_example() {
        let note = Note::from_markdown(MEETING_MD).unwrap();
        assert_eq!(note.id.as_str(), "n_a1b2c3");
        assert_eq!(note.note_type, NoteType::Meeting);
        assert_eq!(note.routing.project(), "Briarwood Golf");
        assert_eq!(note.routing.confidence(), Some(0.94));
        assert_eq!(note.date, "2026-07-09T14:00:00-07:00");
        assert_eq!(note.tags, tags(&["budgeting", "phase-2"]));
        assert!(note.body.starts_with("# Summary"));
        assert!(note.body.ends_with("two alternates."));
    }

    // --- field-omission combinations --------------------------------------

    #[test]
    fn emit_omits_tags_when_empty() {
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "body",
        )
        .unwrap();
        assert!(!note.to_markdown().contains("tags:"));
    }

    #[test]
    fn parse_absent_tags_yields_empty_and_absent_confidence_yields_manual() {
        let md = "---\nid: n_abcdef\ntype: note\nproject: Ops\ndate: 2026-07-10\nsource: manual\n---\n\nbody\n";
        let note = Note::from_markdown(md).unwrap();
        assert!(note.tags.is_empty());
        assert_eq!(
            note.routing,
            Routing::Manual {
                project: "Ops".to_string()
            }
        );
    }

    #[test]
    fn parse_empty_tags_list_reemits_omitted() {
        let md = "---\nid: n_abcdef\ntype: note\nproject: Ops\ndate: 2026-07-10\ntags: []\nsource: manual\n---\n\nbody\n";
        let note = Note::from_markdown(md).unwrap();
        assert!(note.tags.is_empty());
        assert!(!note.to_markdown().contains("tags:"));
    }

    // --- YAML mechanics guards --------------------------------------------

    #[test]
    fn project_named_like_a_scalar_is_quoted_and_round_trips() {
        // Valid folder names that would re-resolve as a non-string scalar
        // (bool / null / number / `~`) unless the serializer quotes them.
        for tricky in ["true", "false", "null", "123", "1.5", "~"] {
            let note = Note::new(
                id(),
                NoteType::Note,
                Routing::Routed {
                    project: tricky.to_string(),
                    confidence: 0.5,
                },
                "2026-07-10",
                vec![],
                source("manual"),
                "body",
            )
            .unwrap();
            let back = Note::from_markdown(&note.to_markdown()).unwrap();
            assert_eq!(
                back.routing.project(),
                tricky,
                "project {tricky:?} must round-trip"
            );
        }
    }

    #[test]
    fn date_shapes_are_emitted_verbatim() {
        for date in [
            "2026-07-10",
            "2026-07-09T14:00:00-07:00",
            "2026-07-11T14:03:00Z",
        ] {
            let note = Note::new(
                id(),
                NoteType::Note,
                Routing::Manual {
                    project: "Ops".to_string(),
                },
                date,
                vec![],
                source("manual"),
                "body",
            )
            .unwrap();
            assert!(note.to_markdown().contains(&format!("date: {date}\n")));
            assert_eq!(Note::from_markdown(&note.to_markdown()).unwrap().date, date);
        }
    }

    #[test]
    fn confidence_one_emits_with_a_decimal_point() {
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Routed {
                project: "Ops".to_string(),
                confidence: 1.0,
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "body",
        )
        .unwrap();
        assert!(note.to_markdown().contains("confidence: 1.0\n"));
    }

    #[test]
    fn output_has_no_stray_document_markers() {
        let md = meeting_note().to_markdown();
        // Exactly two `---` fence lines, no `...` document-end marker.
        assert_eq!(
            md.match_indices("\n---\n").count() + usize::from(md.starts_with("---\n")),
            2
        );
        assert!(!md.contains("\n...\n"));
    }

    // --- NoteId -----------------------------------------------------------

    #[test]
    fn generated_id_matches_the_schema_regex() {
        let generated = NoteId::generate().unwrap();
        let rest = generated.as_str().strip_prefix("n_").unwrap();
        assert_eq!(rest.len(), NOTE_ID_RANDOM_LEN);
        assert!(rest.bytes().all(|b| ALPHABET.contains(&b)));
        assert!(NoteId::parse(generated.as_str()).is_ok());
    }

    #[test]
    fn two_generated_ids_differ() {
        assert_ne!(NoteId::generate().unwrap(), NoteId::generate().unwrap());
    }

    #[test]
    fn note_id_parse_accepts_valid_and_rejects_malformed() {
        assert!(NoteId::parse("n_a1b2c3").is_ok());
        assert!(NoteId::parse("n_abcdefghijkl").is_ok());
        assert!(NoteId::parse("a1b2c3").is_err()); // no prefix
        assert!(NoteId::parse("n_A1B2C3").is_err()); // uppercase
        assert!(NoteId::parse("n_a1b2").is_err()); // too short (< 6)
        assert!(NoteId::parse("n_a1b2c!").is_err()); // non-base36
        assert!(NoteId::parse("n_").is_err()); // empty random part
    }

    // --- NoteType / Tag / Source ------------------------------------------

    #[test]
    fn note_type_round_trips_and_rejects_unknown() {
        for (variant, spelling) in [
            (NoteType::Meeting, "meeting"),
            (NoteType::Note, "note"),
            (NoteType::Chat, "chat"),
        ] {
            assert_eq!(variant.as_str(), spelling);
            assert_eq!(NoteType::parse(spelling).unwrap(), variant);
        }
        assert!(NoteType::parse("Meeting").is_err());
        assert!(NoteType::parse("todo").is_err());
    }

    #[test]
    fn tag_parse_enforces_kebab_case() {
        assert!(Tag::parse("phase-2").is_ok());
        assert!(Tag::parse("budgeting").is_ok());
        assert!(Tag::parse("Phase-2").is_err()); // uppercase
        assert!(Tag::parse("#idea").is_err()); // leading #
        assert!(Tag::parse("-lead").is_err()); // leading dash
        assert!(Tag::parse("double--dash").is_err());
        assert!(Tag::parse("").is_err());
    }

    #[test]
    fn source_distinguishes_keyword_from_path_and_rejects_absolute() {
        assert_eq!(
            Source::parse("quick-capture").unwrap(),
            Source::Keyword(SourceKeyword::QuickCapture)
        );
        assert!(matches!(
            Source::parse("raw/2026.jsonl").unwrap(),
            Source::RawArtifact(_)
        ));
        assert!(Source::parse("/etc/passwd").is_err()); // posix absolute
        assert!(Source::parse("C:/notes/x.jsonl").is_err()); // drive letter
        assert!(Source::parse("").is_err());
    }

    #[test]
    fn source_rejects_backslashes_and_traversal() {
        // A leading-dot filename segment is fine (not a `.`/`..` segment).
        assert!(matches!(
            Source::parse("raw/.keep.jsonl").unwrap(),
            Source::RawArtifact(_)
        ));
        assert!(Source::parse("raw\\2026.jsonl").is_err()); // windows separator
        assert!(Source::parse("\\\\server\\share\\x.jsonl").is_err()); // UNC prefix
        assert!(Source::parse("raw/../../etc/passwd.jsonl").is_err()); // `..` traversal
        assert!(Source::parse("raw/./2026.jsonl").is_err()); // `.` segment
    }

    // --- folder routing & filenames ---------------------------------------

    #[test]
    fn writes_into_the_project_folder_with_a_title_slug() {
        let dir = tempdir().unwrap();
        let path = write_note(dir.path(), &meeting_note(), Some("Weekly Sync!")).unwrap();
        assert_eq!(
            path,
            dir.path().join("Briarwood Golf").join("weekly-sync.md")
        );
        assert_eq!(
            Note::from_markdown(&fs::read_to_string(&path).unwrap()).unwrap(),
            meeting_note()
        );
    }

    #[test]
    fn hierarchical_project_creates_nested_dirs() {
        let dir = tempdir().unwrap();
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Growth/Q3".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "body",
        )
        .unwrap();
        let path = write_note(dir.path(), &note, Some("weekly sync")).unwrap();
        assert_eq!(
            path,
            dir.path().join("Growth").join("Q3").join("weekly-sync.md")
        );
        assert!(path.exists());
    }

    #[test]
    fn inbox_note_lands_in_the_inbox_folder() {
        let dir = tempdir().unwrap();
        let path = write_note(dir.path(), &note_inbox(), Some("idea")).unwrap();
        assert_eq!(path, dir.path().join("Inbox").join("idea.md"));
    }

    #[test]
    fn empty_or_unslugifiable_title_falls_back_to_the_id() {
        let dir = tempdir().unwrap();
        let path = write_note(dir.path(), &meeting_note(), None).unwrap();
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "n_a1b2c3.md");

        let dir2 = tempdir().unwrap();
        let path2 = write_note(dir2.path(), &meeting_note(), Some("✨🎧")).unwrap();
        assert_eq!(path2.file_name().unwrap().to_str().unwrap(), "n_a1b2c3.md");
    }

    // --- collision / atomicity --------------------------------------------

    #[test]
    fn second_note_with_the_same_title_never_overwrites_the_first() {
        let dir = tempdir().unwrap();
        let first = write_note(dir.path(), &meeting_note(), Some("sync")).unwrap();
        let second = write_note(dir.path(), &chat_manual(), Some("sync")).unwrap();
        assert_ne!(first, second);
        assert_eq!(second.file_name().unwrap().to_str().unwrap(), "sync-2.md");
        // Both files are intact and distinct.
        assert_eq!(
            Note::from_markdown(&fs::read_to_string(&first).unwrap()).unwrap(),
            meeting_note()
        );
        assert_eq!(
            Note::from_markdown(&fs::read_to_string(&second).unwrap()).unwrap(),
            chat_manual()
        );
    }

    #[test]
    fn colliding_writes_with_an_overlong_title_terminate_and_stay_distinct() {
        let dir = tempdir().unwrap();
        let long = "meeting-title-".repeat(5); // well over the 40-char cap
        let first = write_note(dir.path(), &meeting_note(), Some(&long)).unwrap();
        let second = write_note(dir.path(), &chat_manual(), Some(&long)).unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let dir = tempdir().unwrap();
        write_note(dir.path(), &meeting_note(), Some("sync")).unwrap();
        let stray = fs::read_dir(dir.path().join("Briarwood Golf"))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!stray, "write should not leave a .tmp scratch file behind");
    }

    // --- relocate_note_file (move across folders) -------------------------

    /// The `note_inbox` note rebuilt as a routed note in `project` — what the
    /// re-route flow hands `relocate_note_file` (same id/source/type/date/tags/
    /// body, new project + confidence).
    fn rerouted(project: &str, confidence: f64) -> Note {
        let base = note_inbox();
        Note::new(
            base.id,
            base.note_type,
            Routing::Routed {
                project: project.to_string(),
                confidence,
            },
            base.date,
            base.tags,
            base.source,
            base.body,
        )
        .unwrap()
    }

    #[test]
    fn relocate_preserves_stem_and_content_and_removes_original() {
        let dir = tempdir().unwrap();
        let old_path = write_note(dir.path(), &note_inbox(), Some("Bright Idea")).unwrap();
        assert_eq!(old_path, dir.path().join("Inbox").join("bright-idea.md"));

        let moved = rerouted("Ops", 1.0);
        let target = dir.path().join("Ops");
        let new_path = relocate_note_file(&moved, &old_path, &target).unwrap();

        // Stem preserved, folder changed, original gone.
        assert_eq!(new_path, target.join("bright-idea.md"));
        assert!(!old_path.exists());
        // New content is the routed note, id preserved.
        let on_disk = Note::from_markdown(&fs::read_to_string(&new_path).unwrap()).unwrap();
        assert_eq!(on_disk, moved);
        assert_eq!(on_disk.id, note_inbox().id);
    }

    #[test]
    fn relocate_suffixes_on_stem_collision_in_target() {
        let dir = tempdir().unwrap();
        // A note already occupies the target stem.
        write_note(dir.path(), &chat_manual(), Some("Bright Idea")).unwrap();
        let old_path = write_note(dir.path(), &note_inbox(), Some("Bright Idea")).unwrap();

        let moved = rerouted("Briarwood Golf", 1.0);
        let target = dir.path().join("Briarwood Golf");
        let new_path = relocate_note_file(&moved, &old_path, &target).unwrap();

        assert_eq!(new_path, target.join("bright-idea-2.md"));
        assert!(!old_path.exists());
    }

    #[test]
    fn relocate_leaves_no_temp_file_behind() {
        let dir = tempdir().unwrap();
        let old_path = write_note(dir.path(), &note_inbox(), Some("idea")).unwrap();
        let target = dir.path().join("Ops");
        relocate_note_file(&rerouted("Ops", 1.0), &old_path, &target).unwrap();

        let stray = fs::read_dir(&target)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(
            !stray,
            "relocate should not leave a .tmp scratch file behind"
        );
    }

    // --- save_note_at (edit in place) -------------------------------------

    #[test]
    fn save_note_at_overwrites_in_place_and_round_trips() {
        let dir = tempdir().unwrap();
        let path = write_note(dir.path(), &meeting_note(), Some("sync")).unwrap();

        // Same id/source/routing; edited body and tags (the edit contract).
        let original = meeting_note();
        let edited = Note::new(
            original.id,
            original.note_type,
            original.routing,
            "2026-07-10",
            tags(&["budgeting"]),
            original.source,
            "Rewritten after the follow-up call.",
        )
        .unwrap();

        save_note_at(&path, &edited).unwrap();
        let on_disk = fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, edited.to_markdown());
        assert_eq!(Note::from_markdown(&on_disk).unwrap(), edited);
        // The edit stayed at the same filename — no sibling file appeared.
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn save_note_at_leaves_no_tmp_file_behind() {
        let dir = tempdir().unwrap();
        let path = write_note(dir.path(), &meeting_note(), Some("sync")).unwrap();
        save_note_at(&path, &meeting_note()).unwrap();
        let stray = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!stray, "save should not leave a .tmp scratch file behind");
    }

    #[test]
    fn save_note_at_errors_when_the_file_is_missing_and_creates_nothing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("gone.md");
        let err = save_note_at(&path, &meeting_note()).unwrap_err();
        assert!(matches!(
            err,
            NoteError::Io { ref source, .. } if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(!path.exists(), "a failed save must not create the file");
    }

    // --- body handling ----------------------------------------------------

    #[test]
    fn body_with_internal_and_leading_horizontal_rules_is_preserved() {
        let body = "---\n\nA divider above, and one below.\n\n---\n\nEnd.";
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            body,
        )
        .unwrap();
        let back = Note::from_markdown(&note.to_markdown()).unwrap();
        assert_eq!(back.body, body);
        assert_eq!(back, note);
    }

    #[test]
    fn messy_whitespace_body_is_normalized_and_idempotent() {
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "\n\n  # Heading\n\nbody\n\n\n",
        )
        .unwrap();
        assert_eq!(note.body, "# Heading\n\nbody");
        // md → struct → md is a fixed point.
        let md = note.to_markdown();
        assert_eq!(Note::from_markdown(&md).unwrap().to_markdown(), md);
    }

    #[test]
    fn empty_body_serializes_to_frontmatter_only() {
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "",
        )
        .unwrap();
        let md = note.to_markdown();
        assert!(md.ends_with("---\n"));
        assert!(!md.contains("---\n\n"));
        assert_eq!(Note::from_markdown(&md).unwrap(), note);
    }

    // --- parse & validation errors ----------------------------------------

    #[test]
    fn missing_or_unterminated_frontmatter_errors() {
        assert!(matches!(
            Note::from_markdown("no frontmatter here"),
            Err(NoteError::MissingFrontmatter)
        ));
        assert!(matches!(
            Note::from_markdown("---\nid: n_a1b2c3\n(no closing fence)\n"),
            Err(NoteError::MissingFrontmatter)
        ));
    }

    #[test]
    fn malformed_yaml_and_missing_fields_error() {
        assert!(matches!(
            Note::from_markdown("---\nid: [\n---\nbody\n"),
            Err(NoteError::Yaml { .. })
        ));
        // Missing required `source`.
        assert!(matches!(
            Note::from_markdown(
                "---\nid: n_a1b2c3\ntype: note\nproject: Ops\ndate: 2026-07-10\n---\nbody\n"
            ),
            Err(NoteError::Yaml { .. })
        ));
    }

    #[test]
    fn invalid_fields_are_rejected_on_parse() {
        let base = |field_lines: &str| format!("---\n{field_lines}---\n\nbody\n");

        let bad_id = base("id: nope\ntype: note\nproject: Ops\ndate: 2026-07-10\nsource: manual\n");
        assert!(matches!(
            Note::from_markdown(&bad_id),
            Err(NoteError::InvalidField { field: "id", .. })
        ));

        let bad_type =
            base("id: n_a1b2c3\ntype: todo\nproject: Ops\ndate: 2026-07-10\nsource: manual\n");
        assert!(matches!(
            Note::from_markdown(&bad_type),
            Err(NoteError::InvalidField { field: "type", .. })
        ));

        let bad_date = base(
            "id: n_a1b2c3\ntype: note\nproject: Ops\ndate: 2026-07-09T14:00:00\nsource: manual\n",
        );
        assert!(matches!(
            Note::from_markdown(&bad_date),
            Err(NoteError::InvalidField { field: "date", .. })
        ));

        let bad_conf = base("id: n_a1b2c3\ntype: note\nproject: Ops\ndate: 2026-07-10\nsource: manual\nconfidence: 1.5\n");
        assert!(matches!(
            Note::from_markdown(&bad_conf),
            Err(NoteError::InvalidField {
                field: "confidence",
                ..
            })
        ));

        let inbox_no_conf = base(
            "id: n_a1b2c3\ntype: note\nproject: Inbox\ndate: 2026-07-10\nsource: quick-capture\n",
        );
        assert!(matches!(
            Note::from_markdown(&inbox_no_conf),
            Err(NoteError::InvalidField {
                field: "confidence",
                ..
            })
        ));
    }

    #[test]
    fn invalid_project_slugs_are_rejected() {
        for bad in [
            "/leading",
            "trailing/",
            "a//b",
            "back\\slash",
            "has:colon",
            "CON",
            "trailingdot.",
        ] {
            let note = Note::new(
                id(),
                NoteType::Note,
                Routing::Manual {
                    project: bad.to_string(),
                },
                "2026-07-10",
                vec![],
                source("manual"),
                "body",
            );
            assert!(note.is_err(), "project {bad:?} should be rejected");
        }
    }

    #[test]
    fn real_project_named_inbox_in_any_casing_is_rejected() {
        // The exact-string `Inbox` sentinel is legal (it is the unfiled marker),
        // but a real project that would share the `<vault>/Inbox/` folder on a
        // case-insensitive filesystem must be rejected.
        for reserved in ["inbox", "INBOX", "InBox", "Inbox/2026", "inbox/sub"] {
            let note = Note::new(
                id(),
                NoteType::Note,
                Routing::Routed {
                    project: reserved.to_string(),
                    confidence: 0.9,
                },
                "2026-07-10",
                vec![],
                source("manual"),
                "body",
            );
            assert!(
                matches!(
                    note,
                    Err(NoteError::InvalidField {
                        field: "project",
                        ..
                    })
                ),
                "project {reserved:?} should be rejected as a reserved Inbox folder"
            );
        }
    }

    #[test]
    fn real_project_named_raw_at_root_is_rejected_but_nested_raw_is_fine() {
        // `<vault>/raw/` holds raw session artifacts; a first segment that
        // case-folds to it would mix routed notes into the sessions tree.
        for reserved in ["raw", "Raw", "RAW", "raw/2026", "Raw/sub"] {
            let note = Note::new(
                id(),
                NoteType::Note,
                Routing::Manual {
                    project: reserved.to_string(),
                },
                "2026-07-10",
                vec![],
                source("manual"),
                "body",
            );
            assert!(
                matches!(
                    note,
                    Err(NoteError::InvalidField {
                        field: "project",
                        ..
                    })
                ),
                "project {reserved:?} should be rejected as the reserved raw folder"
            );
        }
        // Only the vault root is reserved — a nested `raw` is a real project.
        let nested = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Data/raw".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "body",
        );
        assert!(nested.is_ok(), "nested Data/raw should be a legal project");
    }

    #[test]
    fn infra_prefixed_project_segments_are_rejected() {
        // Discovery skips dot-/underscore-prefixed folders at every depth, so
        // the writer must refuse to create projects routing can never see.
        for bad in [
            "_assets",
            ".planning",
            "Growth/_attachments",
            "Growth/.hidden",
        ] {
            let note = Note::new(
                id(),
                NoteType::Note,
                Routing::Manual {
                    project: bad.to_string(),
                },
                "2026-07-10",
                vec![],
                source("manual"),
                "body",
            );
            assert!(
                matches!(
                    note,
                    Err(NoteError::InvalidField {
                        field: "project",
                        ..
                    })
                ),
                "project {bad:?} should be rejected as an infra-prefixed segment"
            );
        }
    }

    #[test]
    fn overlong_project_is_rejected() {
        let too_long = "a".repeat(MAX_PROJECT_LEN + 1);
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual { project: too_long },
            "2026-07-10",
            vec![],
            source("manual"),
            "body",
        );
        assert!(matches!(
            note,
            Err(NoteError::InvalidField {
                field: "project",
                ..
            })
        ));
    }

    #[test]
    fn manual_note_in_inbox_is_rejected() {
        let note = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: INBOX.to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "body",
        );
        assert!(matches!(
            note,
            Err(NoteError::InvalidField {
                field: "project",
                ..
            })
        ));
    }

    #[test]
    fn reserved_root_dir_project_in_any_casing_is_rejected() {
        // These root dirs belong to other subsystems and are skipped by vault
        // enumeration — a note filed there would be unreachable in the UI.
        for reserved in ["sessions", "Sessions", "raw", "RAW", "EBWebView", "raw/sub"] {
            let note = Note::new(
                id(),
                NoteType::Note,
                Routing::Manual {
                    project: reserved.to_string(),
                },
                "2026-07-10",
                vec![],
                source("manual"),
                "body",
            );
            assert!(
                matches!(
                    note,
                    Err(NoteError::InvalidField {
                        field: "project",
                        ..
                    })
                ),
                "project {reserved:?} should be rejected as a reserved root dir"
            );
        }
    }

    #[test]
    fn tag_parse_normalized_folds_case_and_trims_but_stays_strict_otherwise() {
        assert_eq!(
            Tag::parse_normalized(" Phase-2 ").unwrap(),
            Tag::parse("phase-2").unwrap()
        );
        assert!(Tag::parse("Phase-2").is_err()); // frontmatter stays strict
        assert!(Tag::parse_normalized("bad tag").is_err());
        assert!(Tag::parse_normalized("").is_err());
    }

    #[test]
    fn with_edits_preserves_id_source_and_routing_and_replaces_editable_fields() {
        let original = Note::new(
            id(),
            NoteType::Meeting,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 0.94,
            },
            "2026-07-09T14:00:00-07:00",
            vec![Tag::parse("budgeting").unwrap()],
            source("raw/20260709T210000000Z-k4m2xp7q-sync.jsonl"),
            "Original body.",
        )
        .unwrap();

        let merged = original
            .clone()
            .with_edits(NoteEdit {
                note_type: NoteType::Note,
                date: "2026-07-12".to_string(),
                tags: vec![Tag::parse("follow-up").unwrap()],
                body: "Rewritten.".to_string(),
            })
            .unwrap();

        // Preserved verbatim — including the routing score an edit can't carry.
        assert_eq!(merged.id, original.id);
        assert_eq!(merged.routing, original.routing);
        assert_eq!(merged.source, original.source);
        // Replaced from the edit.
        assert_eq!(merged.note_type, NoteType::Note);
        assert_eq!(merged.date, "2026-07-12");
        assert_eq!(merged.tags, vec![Tag::parse("follow-up").unwrap()]);
        assert_eq!(merged.body, "Rewritten.");
    }

    #[test]
    fn with_edits_keeps_a_manual_note_confidence_free() {
        let manual = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "Body.",
        )
        .unwrap();
        let merged = manual
            .with_edits(NoteEdit {
                note_type: NoteType::Note,
                date: "2026-07-11".to_string(),
                tags: vec![],
                body: "New.".to_string(),
            })
            .unwrap();
        assert_eq!(
            merged.routing,
            Routing::Manual {
                project: "Ops".to_string()
            }
        );
        assert!(!merged.to_markdown().contains("confidence:"));
    }

    #[test]
    fn with_edits_revalidates_the_result() {
        let original = Note::new(
            id(),
            NoteType::Note,
            Routing::Manual {
                project: "Ops".to_string(),
            },
            "2026-07-10",
            vec![],
            source("manual"),
            "Body.",
        )
        .unwrap();
        let bad_date = original.with_edits(NoteEdit {
            note_type: NoteType::Note,
            date: "2026-07-10T14:00:00".to_string(), // offset-less: invalid
            tags: vec![],
            body: String::new(),
        });
        assert!(matches!(
            bad_date,
            Err(NoteError::InvalidField { field: "date", .. })
        ));
    }
}
