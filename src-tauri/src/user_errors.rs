//! The translation seam between a core error's `Display` and the sentence a
//! `#[tauri::command]` hands the frontend.
//!
//! `docs/DESIGN_SYSTEM.md` §3: an error "names what failed and what happens
//! next, in the user's words. It never leaks an exception string, and it always
//! leaves the underlying data reachable." A core `Display` cannot do that job
//! and its own: `NoteError::Io` carries the absolute path and the OS error text
//! *on purpose*, because the MCP server's caller is a developer who wants them.
//! So the wrapper layer translates instead, and the core strings stay untouched
//! for MCP. See `.claude/rules/tauri-command-parity.md` layer 2.
//!
//! Two shapes cross this seam:
//!
//! - **Replaced** — I/O, YAML, poisoned locks, anything whose text is a machine
//!   fact. The raw error goes to stderr, and a per-command sentence goes to the
//!   user. [`reported`] is that pairing, and no command should log-and-return by
//!   hand.
//! - **Passed through** — validation detail (`NoteError::InvalidField`,
//!   `GlossaryOpError::InvalidInput`) and the conflict cases. These describe
//!   what the *user* typed, so they are already the user's words; they get
//!   sentence-cased by [`user_sentence`] and nothing else.
//!
//! One caveat worth knowing: a release GUI launch has no attached console, so
//! the `eprintln!`s here are discarded unless the app was started from a
//! terminal. That is the accepted cost of not taking a logging dependency (this
//! workspace has none, and ~70 background call sites already use `eprintln!`);
//! the alternative was leaking the detail into the UI, which is what this module
//! exists to stop.

use kodabi_core::glossary::GlossaryError;
use kodabi_core::note::NoteError;
use kodabi_core::vault::GlossaryOpError;

/// Logs `err` under the command's name, then returns the sentence the user
/// reads. The one place the raw detail and the user's copy part ways.
pub(crate) fn reported(cmd: &str, err: impl std::fmt::Display, sentence: &str) -> String {
    eprintln!("{cmd}: {err}");
    sentence.to_string()
}

/// Renders a core validation detail as a sentence: capitalized, and terminated.
///
/// Core writes these lowercase and unterminated because they read as a clause
/// after `Display`'s `"invalid note {field}: "` prefix. The wrapper drops that
/// prefix (it names a Rust field, not anything the user typed), which leaves the
/// clause standing alone as the whole message.
pub(crate) fn user_sentence(detail: &str) -> String {
    let trimmed = detail.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut chars = trimmed.chars();
    // `next()` is Some: the emptiness check above already ruled out None.
    let mut sentence: String = chars
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    sentence.push_str(chars.as_str());
    if !sentence.ends_with(['.', '!', '?']) {
        sentence.push('.');
    }
    sentence
}

/// Translates a [`NoteError`] into user copy.
///
/// `replaced` is the caller's sentence for the machine-fact variants (I/O, YAML,
/// a missing frontmatter fence): it names what *that command* was doing and what
/// happens next, which is knowledge only the call site has. The variants that
/// describe the user's own input answer for themselves.
pub(crate) fn note_error(cmd: &str, err: NoteError, replaced: &str) -> String {
    match err {
        NoteError::InvalidField { detail, .. } => user_sentence(&detail),
        NoteError::MissingProject { .. } => {
            "That project does not exist. It may have been renamed or deleted; refresh the list \
             and try again."
                .to_string()
        }
        NoteError::ProjectExists { project } => {
            format!("A project named \"{project}\" already exists. Pick a different name.")
        }
        // The paths belong in the log, not on screen, but the user still has to
        // act on disk: name the conflict without printing a `Vec<PathBuf>`.
        err @ NoteError::DuplicateNoteId { .. } => reported(
            cmd,
            err,
            "Two files in your vault claim this note. Remove one of the duplicates in your vault \
             folder, then try again.",
        ),
        err => reported(cmd, err, replaced),
    }
}

/// Translates a [`GlossaryOpError`] into user copy, on the same split as
/// [`note_error`]: the conflict and validation cases are about what the user
/// typed or what is in their file, and the storage failures are not.
pub(crate) fn glossary_error(cmd: &str, err: GlossaryOpError, replaced: &str) -> String {
    match err {
        GlossaryOpError::InvalidProject(err) => note_error(cmd, err, replaced),
        GlossaryOpError::InvalidInput { detail, .. } => user_sentence(&detail),
        GlossaryOpError::MissingProject { .. } => {
            "That project does not exist. It may have been renamed or deleted; refresh the \
             sidebar."
                .to_string()
        }
        GlossaryOpError::Conflict { term } => {
            format!("A term named \"{term}\" is already in this glossary. Edit the existing entry instead.")
        }
        GlossaryOpError::NotFound { term } => {
            format!("The term \"{term}\" is no longer in this glossary. It may have been removed; reopen this view.")
        }
        GlossaryOpError::Storage(err) => glossary_storage_error(cmd, err, replaced),
    }
}

/// The `_glossary.yml` file's own failures. A malformed or duplicate-bearing
/// file is the user's to fix in an editor, so those name the repair; I/O is a
/// machine fact and takes the caller's sentence.
fn glossary_storage_error(cmd: &str, err: GlossaryError, replaced: &str) -> String {
    match err {
        GlossaryError::Yaml { .. } => reported(
            cmd,
            err,
            "This glossary file isn't valid YAML. Fix the file in a text editor, then reopen this \
             view.",
        ),
        GlossaryError::Duplicate { ref term, .. } => {
            let sentence = format!(
                "This glossary lists \"{term}\" twice. Fix the file in a text editor, then reopen \
                 this view."
            );
            reported(cmd, &err, &sentence)
        }
        GlossaryError::Conflict { term } => {
            format!("A term named \"{term}\" is already in this glossary. Edit the existing entry instead.")
        }
        GlossaryError::Io { .. } => reported(cmd, err, replaced),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// The whole point of the module: the machine facts in an I/O error reach
    /// the log and nothing else. Asserting on absence rather than equality so a
    /// future rewording of the sentence cannot quietly reintroduce the path.
    #[test]
    fn io_error_leaks_neither_path_nor_os_error() {
        let err = NoteError::Io {
            path: PathBuf::from(r"C:\Users\someone\kb\Briarwood Golf\n_a1b2c3.md"),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "os error 5"),
        };
        let message = note_error("save_note", err, "Couldn't save your changes. Try again.");
        assert_eq!(message, "Couldn't save your changes. Try again.");
        assert!(!message.contains("os error"));
        assert!(!message.contains(r"C:\"));
        assert!(!message.contains("Briarwood"));
    }

    /// `DuplicateNoteId` interpolates a `Debug`-formatted `Vec<PathBuf>`, which
    /// is the ugliest string that ever reached the UI. It gets its own sentence
    /// rather than the caller's, because the user has a specific repair to make.
    #[test]
    fn duplicate_note_id_names_the_repair_without_the_paths() {
        let err = NoteError::DuplicateNoteId {
            id: "n_a1b2c3".to_string(),
            paths: vec![PathBuf::from(r"C:\kb\a.md"), PathBuf::from(r"C:\kb\b.md")],
        };
        let message = note_error("read_note", err, "unused");
        assert!(message.starts_with("Two files in your vault claim this note."));
        assert!(!message.contains(r"C:\"));
        assert!(!message.contains('['));
    }

    /// Validation detail is the user's own input described back to them, so it
    /// passes through. The `Display` prefix ("invalid note project: ") names a
    /// Rust field and does not.
    #[test]
    fn validation_detail_passes_through_as_a_sentence() {
        let err = NoteError::InvalidField {
            field: "project",
            detail: "project segment \"aux\" is a reserved Windows device name".to_string(),
        };
        let message = note_error("create_project", err, "unused");
        assert_eq!(
            message,
            "Project segment \"aux\" is a reserved Windows device name."
        );
        assert!(!message.contains("invalid note"));
    }

    #[test]
    fn project_exists_names_the_project_and_the_next_step() {
        let err = NoteError::ProjectExists {
            project: "Briarwood Golf".to_string(),
        };
        let message = note_error("create_project", err, "unused");
        assert_eq!(
            message,
            "A project named \"Briarwood Golf\" already exists. Pick a different name."
        );
    }

    #[test]
    fn missing_project_drops_the_create_project_advice() {
        let err = NoteError::MissingProject {
            project: "Gone".to_string(),
        };
        let message = note_error("file_note_to_project", err, "unused");
        assert!(!message.contains("create_project"));
        assert!(message.contains("refresh the list"));
    }

    /// A glossary file the user can repair says so, and names the term to look
    /// for. The path in the error stays in the log.
    ///
    /// (The sibling `Yaml` arm is not unit-tested here: building a
    /// `serde_yaml_ng::Error` needs that crate as a direct dependency of
    /// `src-tauri`, and adding one for a test is a worse trade than covering the
    /// arm beside it, which takes the identical log-and-replace path.)
    #[test]
    fn glossary_duplicate_names_the_repair_without_the_path() {
        let err = GlossaryOpError::Storage(GlossaryError::Duplicate {
            path: PathBuf::from(r"C:\Users\someone\kb\_glossary.yml"),
            term: "MERIDIAN".to_string(),
        });
        let message = glossary_error("list_glossary", err, "unused");
        assert!(message.contains("\"MERIDIAN\" twice"));
        assert!(message.contains("text editor"));
        assert!(!message.contains(r"C:\"));
    }

    #[test]
    fn glossary_conflict_names_the_term() {
        let err = GlossaryOpError::Conflict {
            term: "MERIDIAN".to_string(),
        };
        let message = glossary_error("add_glossary_term", err, "unused");
        assert!(message.contains("\"MERIDIAN\""));
        assert!(message.contains("Edit the existing entry instead."));
    }

    #[test]
    fn user_sentence_capitalizes_and_terminates_once() {
        assert_eq!(
            user_sentence("project must be short"),
            "Project must be short."
        );
        assert_eq!(user_sentence("Already done."), "Already done.");
        assert_eq!(user_sentence("really?"), "Really?");
        assert_eq!(user_sentence("  padded  "), "Padded.");
        assert_eq!(user_sentence(""), "");
    }
}
