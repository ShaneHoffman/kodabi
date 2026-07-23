//! Quick capture: raw jotted text → a routed `type: note` note on disk, in one
//! call. The manual-note counterpart to the transcript
//! [`distill`](crate::distill) pass — but with no model call. A quick-captured
//! thought is short and already in the user's words, so it is filed verbatim:
//! the same lexical [`routing`](crate::routing) engine that files a distilled
//! meeting decides where it lands (a project when confidence clears the
//! threshold, else [`INBOX`](crate::note::INBOX) with the score recorded), and
//! the body is written untouched.
//!
//! Pure and UI-agnostic, like the rest of `kodabi-core`: the Tauri shell
//! resolves the vault root, mints the date, and applies any
//! `KODABI_ROUTING_THRESHOLD` override before calling in.

use std::io;
use std::path::{Path, PathBuf};

use crate::note::{self, Note, NoteError, NoteId, NoteType, Source, SourceKeyword};
use crate::routing::{
    self, ExamplesLoadFailure, GlossaryLoadFailure, NoteText, RoutingConfig, RoutingError,
};

/// Errors produced while quick-capturing a note.
#[derive(Debug, thiserror::Error)]
pub enum QuickCaptureError {
    /// The submitted text was empty or whitespace-only — nothing to capture.
    #[error("quick capture text is empty")]
    EmptyBody,
    /// The OS RNG was unavailable while minting the note id.
    #[error("failed to generate note id: {0}")]
    IdGeneration(#[source] io::Error),
    #[error(transparent)]
    Routing(#[from] RoutingError),
    #[error(transparent)]
    Note(#[from] NoteError),
}

/// A quick-captured note: the validated [`Note`] and the path it was written to.
pub struct QuickCaptured {
    pub note: Note,
    pub path: PathBuf,
    /// Projects whose glossary could not load. Routing proceeded treating each
    /// as having no vocabulary (they still match their own name), so this is a
    /// non-fatal "fix your glossary" report, empty on the happy path. See
    /// [`GlossaryLoadFailure`].
    pub glossary_failures: Vec<GlossaryLoadFailure>,
    /// Projects whose routing-examples log could not load. Routing proceeded
    /// treating each as having no recorded corrections (they still match on name
    /// and glossary), so this is a non-fatal "fix your corrections log" report,
    /// empty on the happy path. See [`ExamplesLoadFailure`].
    pub example_failures: Vec<ExamplesLoadFailure>,
}

/// Files `body` as a routed `type: note`, `source: quick-capture` note under
/// `vault_root`, returning the note and the path it landed at.
///
/// `body` is trimmed and rejected when empty; `date` is the caller-minted filing
/// date (ISO-8601, typically a date-only `YYYY-MM-DD` — a jotted thought has no
/// meaningful clock time). Routing is purely lexical, no model call:
/// [`route`](routing::route) files the note into the best-matching project when
/// confidence clears `config`'s threshold, else into [`INBOX`](note::INBOX) with
/// the score recorded. The body is written verbatim; the filename is seeded from
/// its first non-empty line (falling back to the note id when that line has no
/// slug-safe characters), so an Inbox listing shows a readable title. That same
/// first line is also stored as the note's frontmatter `title`, so a long first
/// line shows in full rather than being cut at the 40-char filename slug.
pub fn quick_capture(
    vault_root: &Path,
    body: &str,
    date: &str,
    config: &RoutingConfig,
) -> Result<QuickCaptured, QuickCaptureError> {
    let body = body.trim();
    if body.is_empty() {
        return Err(QuickCaptureError::EmptyBody);
    }

    // Route on the raw text: `route` takes plain title/body, so the distill pass
    // is skipped entirely. Title is `None` — a quick capture has no separate
    // title; the first body line only seeds the filename, never a routing
    // signal (that would double-count the same words as both title and body).
    let loaded = routing::load_project_signals(vault_root)?;
    let routing = routing::route(NoteText { title: None, body }, &loaded.signals, config);

    let id = NoteId::generate().map_err(QuickCaptureError::IdGeneration)?;
    // The first body line seeds both the filename slug and the stored title.
    // Storing it (normalized, ≤120 chars) means a long first line shows in full
    // instead of being cut at the 40-char slug. This is a *display* title, not a
    // routing signal — routing still saw `title: None` above, on purpose.
    let seed = filename_seed(body);
    let note = Note::new(
        id,
        NoteType::Note,
        routing,
        date,
        Vec::new(),
        Source::Keyword(SourceKeyword::QuickCapture),
        body,
    )?
    .with_title(seed.map(str::to_string));
    let path = note::write_note(vault_root, &note, seed)?;
    Ok(QuickCaptured {
        note,
        path,
        glossary_failures: loaded.glossary_failures,
        example_failures: loaded.example_failures,
    })
}

/// The first non-empty line of `body`, seeding the note's filename. Returns
/// `None` only for an all-blank body (which [`quick_capture`] rejects before
/// reaching here); [`note::write_note`] itself falls back to the id when the
/// seed slugifies to nothing.
fn filename_seed(body: &str) -> Option<&str> {
    body.lines().map(str::trim).find(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::Routing;
    use tempfile::tempdir;

    /// Parse a note back off disk from a `QuickCaptured` result.
    fn read_back(captured: &QuickCaptured) -> Note {
        let markdown = std::fs::read_to_string(&captured.path).unwrap();
        Note::from_markdown(&markdown).unwrap()
    }

    #[test]
    fn rejects_empty_and_whitespace_body() {
        let vault = tempdir().unwrap();
        for body in ["", "   ", "\n\t \n"] {
            assert!(matches!(
                quick_capture(vault.path(), body, "2026-07-18", &RoutingConfig::default()),
                Err(QuickCaptureError::EmptyBody)
            ));
        }
        // Nothing was written — not even the Inbox folder.
        assert!(!vault.path().join("Inbox").exists());
    }

    #[test]
    fn lands_in_inbox_when_nothing_matches() {
        let vault = tempdir().unwrap();
        let captured = quick_capture(
            vault.path(),
            "A stray thought with no project to call home.",
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();

        assert!(captured.path.starts_with(vault.path().join("Inbox")));
        let note = read_back(&captured);
        assert_eq!(note.note_type, NoteType::Note);
        assert_eq!(note.source, Source::Keyword(SourceKeyword::QuickCapture));
        assert_eq!(
            note.routing,
            Routing::Routed {
                project: note::INBOX.to_string(),
                confidence: 0.0,
            }
        );
    }

    #[test]
    fn stores_the_full_first_line_as_title_past_the_slug_cap() {
        let vault = tempdir().unwrap();
        let long_first_line =
            "remember to ask priya about the clubhouse drainage contractor next sync";
        assert!(long_first_line.len() > 40);
        let captured = quick_capture(
            vault.path(),
            &format!("{long_first_line}\n\nmore detail below"),
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();

        // Filename slug is still capped for the filesystem...
        let stem = captured.path.file_stem().unwrap().to_str().unwrap();
        assert!(stem.chars().count() <= 40);
        // ...but the stored title is the full first line, uncut.
        let note = read_back(&captured);
        assert_eq!(note.title.as_deref(), Some(long_first_line));
    }

    #[test]
    fn files_into_a_project_that_clears_the_threshold() {
        let vault = tempdir().unwrap();
        std::fs::create_dir(vault.path().join("Paradise Golf")).unwrap();

        // A lone name mention scores saturate(2.0) = 0.5, which clears a
        // lowered threshold but not the 0.6 default (by design — projects get
        // name-dropped in other projects' notes). The lowered config keeps this
        // test off glossary-file coupling.
        let captured = quick_capture(
            vault.path(),
            "Met the Paradise Golf team about the irrigation bid.",
            "2026-07-18",
            &RoutingConfig { threshold: 0.4 },
        )
        .unwrap();

        assert!(captured
            .path
            .starts_with(vault.path().join("Paradise Golf")));
        assert_eq!(
            read_back(&captured).routing,
            Routing::Routed {
                project: "Paradise Golf".to_string(),
                confidence: 0.5,
            }
        );
    }

    #[test]
    fn body_round_trips_verbatim_modulo_trim() {
        let vault = tempdir().unwrap();
        let captured = quick_capture(
            vault.path(),
            "\n  Buy milk\nand call the dentist.\n\n",
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();

        assert_eq!(read_back(&captured).body, "Buy milk\nand call the dentist.");
    }

    #[test]
    fn filename_derives_from_the_first_line() {
        let vault = tempdir().unwrap();
        let captured = quick_capture(
            vault.path(),
            "Ship the quick capture window\nfollow-up details here",
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();

        assert_eq!(
            captured.path.file_name().unwrap().to_str().unwrap(),
            "ship-the-quick-capture-window.md"
        );
    }

    #[test]
    fn filename_falls_back_to_id_when_the_first_line_has_no_slug() {
        let vault = tempdir().unwrap();
        let captured = quick_capture(
            vault.path(),
            "!!!\nthe real thought is on the next line",
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();

        let stem = captured.path.file_stem().unwrap().to_str().unwrap();
        assert_eq!(stem, captured.note.id.as_str());
    }

    #[test]
    fn mints_a_fresh_id_that_matches_the_written_file() {
        let vault = tempdir().unwrap();
        let captured = quick_capture(
            vault.path(),
            "an idea worth keeping",
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();

        assert_eq!(read_back(&captured).id, captured.note.id);
    }

    #[test]
    fn quick_capture_learns_from_recorded_corrections() {
        use crate::routing_examples::{RoutingExample, RoutingExamples};

        let vault = tempdir().unwrap();
        let project_dir = vault.path().join("Paradise Golf");
        std::fs::create_dir(&project_dir).unwrap();

        // A prior correction about a clubhouse migration is on record for the
        // project (no glossary involved).
        let mut log = RoutingExamples::default();
        log.upsert(RoutingExample {
            note_id: "n_seed01".to_string(),
            title: "Clubhouse migration".to_string(),
            excerpt: "clubhouse migration cutover plan".to_string(),
            previous_project: None,
            confidence: 1.0,
            corrected_at: "2026-07-18T20:15:00Z".to_string(),
            reason: None,
        });
        log.save(&project_dir).unwrap();

        // A new capture about the same topic files itself: name (2.0) + a perfect
        // example match (2.0) = 4.0, saturate(4) = 4/6, clearing the 0.6 default.
        let captured = quick_capture(
            vault.path(),
            "Paradise Golf clubhouse migration cutover plan",
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();
        assert!(captured.path.starts_with(&project_dir));
        assert_eq!(
            read_back(&captured).routing,
            Routing::Routed {
                project: "Paradise Golf".to_string(),
                confidence: 4.0 / 6.0,
            }
        );

        // An unrelated capture is untouched by the correction.
        let unrelated = quick_capture(
            vault.path(),
            "Buy groceries and call the plumber",
            "2026-07-18",
            &RoutingConfig::default(),
        )
        .unwrap();
        assert!(unrelated.path.starts_with(vault.path().join("Inbox")));
        assert_eq!(
            read_back(&unrelated).routing,
            Routing::Routed {
                project: note::INBOX.to_string(),
                confidence: 0.0,
            }
        );
    }
}
