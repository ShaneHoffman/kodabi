//! Reading the `chats/` store from a note's point of view: which chat
//! transcripts have not yet become a `type: chat` note.
//!
//! The same derived-membership trick [`crate::sessions`] uses, for the same
//! reason — markdown is the truth, so nothing on disk records "this chat was
//! distilled":
//!
//! ```text
//! undistilled = <vault>/chats/*.jsonl
//!             − chats some note already claims via its `source:` field
//!             − transcripts still being written (see `settled_before`)
//!             − chats too thin to be worth a note
//! ```
//!
//! Deriving it is what lets a chat distill survive an app crash or a quit: a
//! session that ended with the app never got a hook, and the next launch's
//! sweep simply finds it unclaimed. It self-heals the same way too — a
//! transcript the user deletes stops being listed.
//!
//! Unlike `sessions/`, an undistilled chat is **never surfaced to the user**.
//! There is no needs-attention row for it and no retry button: `chats/` is a
//! deliberate sibling of `sessions/` precisely so a chat can't read as an
//! unclaimed capture ([`crate::chat::CHATS_DIR`]). A chat that fails to distill
//! is simply retried on the next launch, silently, forever — which is the right
//! trade when the transcript itself is never destroyed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use crate::chat::{self, CHATS_DIR};
use crate::note::NoteError;
use crate::vault;

/// Why deriving the undistilled-chat list failed outright.
///
/// Per-file trouble never lands here: an unreadable transcript is skipped (it
/// would fail the same way in the distill, and there is no surface to report it
/// on), and an unparseable note is skipped by
/// [`vault::collect_raw_artifact_sources`]. This is only a chats directory or
/// vault root that can't be enumerated at all.
#[derive(Debug, thiserror::Error)]
pub enum ChatsError {
    #[error("failed to read chats directory {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Notes(#[from] NoteError),
}

/// Lists the chat transcripts no note claims yet, oldest first.
///
/// `settled_before` excludes anything modified after it, which is what keeps
/// the sweep off a transcript that is still being appended to. The caller
/// passes "now minus a grace window"; distilling a live chat is the one failure
/// that cannot be retried, because the note would claim the `source:` and the
/// rest of the conversation would never be seen again.
///
/// Cost is linear in the size of the vault: one walk that parses every note to
/// collect the claimed set (the same class of walk as
/// [`crate::sessions::list_failed_sessions`]), plus a full read of each
/// unclaimed transcript to apply [`chat::has_distillable_substance`]. Unlike
/// the needs-attention list this runs once per launch rather than when a view
/// opens — acceptable for a personal vault, where `chats/` holds tens of small
/// files.
pub fn list_undistilled_chats(
    vault_root: &Path,
    settled_before: DateTime<Utc>,
) -> Result<Vec<PathBuf>, ChatsError> {
    let claimed = vault::collect_raw_artifact_sources(vault_root)?;
    let chats_dir = vault_root.join(CHATS_DIR);

    let entries = match fs::read_dir(&chats_dir) {
        Ok(entries) => entries,
        // No chat has ever been opened: an empty list, not an error.
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ChatsError::Io {
                path: chats_dir,
                source,
            })
        }
    };

    let mut undistilled = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        // Regular top-level `.jsonl` files only, mirroring the sessions walk.
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if claimed.contains(&source_value(file_name)) {
            continue;
        }
        if !is_settled(&entry, settled_before) {
            continue;
        }
        // An unreadable transcript is skipped rather than listed: the distill
        // would fail on it identically, and there is no surface to report that
        // on. It stays on disk, so a transient read failure retries next launch.
        let Ok(records) = chat::read_transcript(&path) else {
            continue;
        };
        if !chat::has_distillable_substance(&records) {
            continue;
        }
        undistilled.push(path);
    }

    // Oldest first: the filename scheme is timestamp-led, so a lexical sort is
    // chronological, and a sweep that distills in order matches the sequence
    // the conversations actually happened in.
    undistilled.sort();
    Ok(undistilled)
}

/// Whether a transcript has stopped being written to, as far as the filesystem
/// can say. A file whose mtime can't be read is treated as unsettled — the
/// cautious direction, since the cost is one deferred sweep against a note that
/// claims a conversation still in progress.
fn is_settled(entry: &fs::DirEntry, settled_before: DateTime<Utc>) -> bool {
    entry
        .metadata()
        .and_then(|meta| meta.modified())
        .map(|modified| DateTime::<Utc>::from(modified) <= settled_before)
        .unwrap_or(false)
}

/// The `source:` value a note carries for the chat named `file_name` —
/// vault-relative with forward slashes, exactly the form
/// [`crate::chat_distill::distill_chat`] writes.
fn source_value(file_name: &str) -> String {
    format!("{CHATS_DIR}/{file_name}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{ChatRecord, ChatTranscript};
    use crate::device::DeviceId;
    use crate::note::{Note, NoteId, NoteType, Routing, Source, Tag};
    use chrono::TimeZone;

    /// Far enough ahead that every fixture counts as settled.
    fn later() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap()
    }

    fn long(prefix: &str) -> String {
        format!("{prefix} {}", "detail ".repeat(60))
    }

    fn substantive() -> Vec<ChatRecord> {
        vec![
            ChatRecord::Meta {
                chat_id: "3c9a35a1".to_string(),
                model: "sonnet".to_string(),
                started_at: "2026-07-10T16:15:00Z".to_string(),
            },
            ChatRecord::User {
                ts: "2026-07-10T16:15:40Z".to_string(),
                text: long("Which contractor?"),
            },
            ChatRecord::Assistant {
                ts: "2026-07-10T16:15:45Z".to_string(),
                text: long("GreenFlow."),
            },
        ]
    }

    fn write_chat(vault: &Path, seconds: u32, records: &[ChatRecord]) -> PathBuf {
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let started = Utc.with_ymd_and_hms(2026, 7, 10, 16, 15, seconds).unwrap();
        let transcript = ChatTranscript::create(vault, &device, started).unwrap();
        for record in records {
            transcript.append(record).unwrap();
        }
        transcript.path().to_path_buf()
    }

    /// Files a note whose `source:` claims `chat_path`.
    fn claim(vault: &Path, chat_path: &Path) {
        let file_name = chat_path.file_name().unwrap().to_str().unwrap();
        let note = Note::new(
            NoteId::generate().unwrap(),
            NoteType::Chat,
            Routing::Routed {
                project: "Briarwood Golf".to_string(),
                confidence: 0.9,
            },
            "2026-07-10T09:15:00-07:00",
            Vec::<Tag>::new(),
            Source::parse(&format!("{CHATS_DIR}/{file_name}")).unwrap(),
            "# Summary\n\nCompared the bidders.",
        )
        .unwrap();
        crate::note::write_note(vault, &note, Some("contractor-comparison")).unwrap();
    }

    #[test]
    fn lists_a_chat_no_note_claims() {
        let vault = tempfile::tempdir().unwrap();
        let chat_path = write_chat(vault.path(), 0, &substantive());

        assert_eq!(
            list_undistilled_chats(vault.path(), later()).unwrap(),
            vec![chat_path]
        );
    }

    #[test]
    fn excludes_a_chat_claimed_by_a_note_source() {
        let vault = tempfile::tempdir().unwrap();
        let chat_path = write_chat(vault.path(), 0, &substantive());
        claim(vault.path(), &chat_path);

        assert!(list_undistilled_chats(vault.path(), later())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn excludes_a_thin_chat() {
        let vault = tempfile::tempdir().unwrap();
        write_chat(
            vault.path(),
            0,
            &[
                ChatRecord::User {
                    ts: "2026-07-10T16:15:40Z".to_string(),
                    text: "hi".to_string(),
                },
                ChatRecord::Assistant {
                    ts: "2026-07-10T16:15:45Z".to_string(),
                    text: "hello".to_string(),
                },
            ],
        );

        assert!(list_undistilled_chats(vault.path(), later())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn excludes_a_transcript_still_being_written() {
        let vault = tempfile::tempdir().unwrap();
        write_chat(vault.path(), 0, &substantive());

        // A grace window that ends before the file was touched: the live
        // session's transcript must never be swept.
        let too_early = Utc.with_ymd_and_hms(2000, 1, 1, 0, 0, 0).unwrap();

        assert!(list_undistilled_chats(vault.path(), too_early)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn missing_chats_dir_yields_empty() {
        let vault = tempfile::tempdir().unwrap();

        assert!(list_undistilled_chats(vault.path(), later())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn ignores_non_jsonl_and_subdirectories() {
        let vault = tempfile::tempdir().unwrap();
        let kept = write_chat(vault.path(), 0, &substantive());
        let dir = vault.path().join(CHATS_DIR);
        fs::write(dir.join("notes.txt"), "not a transcript").unwrap();
        fs::create_dir(dir.join("nested")).unwrap();

        assert_eq!(
            list_undistilled_chats(vault.path(), later()).unwrap(),
            vec![kept]
        );
    }

    #[test]
    fn lists_oldest_first() {
        let vault = tempfile::tempdir().unwrap();
        let first = write_chat(vault.path(), 0, &substantive());
        let second = write_chat(vault.path(), 30, &substantive());

        assert_eq!(
            list_undistilled_chats(vault.path(), later()).unwrap(),
            vec![first, second]
        );
    }
}
