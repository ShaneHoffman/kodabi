//! Persists a captured session's transcript to disk as newline-delimited
//! JSON (JSONL) — the durable, file-as-source-of-truth record a session
//! produces before Phase 2 distills it into a markdown note (FOUNDING_DOC
//! §3.4, §3.6). Filenames follow the locked scheme in
//! [`crate::naming::session_filename`]; the SQLite index (Phase 2) is
//! derived from these files, never the other way around (§3.6).
//!
//! Each line is one [`TranscriptSegment`], matching the wire shape in
//! `docs/MCP_TOOL_SURFACE.md` field-for-field so a Phase 2 reader needs no
//! translation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::device::DeviceId;
use crate::naming::{numbered_slug, session_dir_name, session_filename};
use crate::transcription::{Channel, Segment};

/// Per-process counter that, combined with the process id, gives each
/// in-flight write a unique temp filename so concurrent writes can't
/// clobber each other's scratch file (mirrors `glossary.rs`).
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// One line of a persisted transcript: a [`Segment`] plus the attribution
/// (`index`, `channel`, `speaker`) outer layers add before writing (see
/// `transcription`'s module docs). Mirrors `docs/MCP_TOOL_SURFACE.md`'s
/// `TranscriptSegment` schema field-for-field, in the same field order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    /// 0-based ordinal within the transcript.
    pub index: u64,
    /// Which side of the conversation this segment came from.
    pub channel: Channel,
    /// Speaker label if known; always `None` in v1 (diarization within a
    /// channel is post-v1 — see [`Channel`]'s docs). Serialized as `null`
    /// rather than omitted, for exact parity with the documented schema.
    #[serde(default)]
    pub speaker: Option<String>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

/// Errors produced while writing or reading a raw session transcript.
#[derive(Debug, thiserror::Error)]
pub enum RawSessionError {
    #[error("raw session I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("raw session JSON error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

/// `Result` specialised to [`RawSessionError`].
pub type Result<T> = std::result::Result<T, RawSessionError>;

/// Merges per-channel segment lists into one transcript ordered by
/// `start_ms`, assigning 0-based indices.
///
/// This is the transcript-level counterpart to `kodabi_audio::combine`'s
/// PCM-level two-channel alignment — the you/them attribution actually
/// lands on the text here. The sort is stable, so when two segments from
/// different channels share a `start_ms` the result stays deterministic
/// (each channel's own segments keep their relative order).
pub fn assemble(
    channels: impl IntoIterator<Item = (Channel, Vec<Segment>)>,
) -> Vec<TranscriptSegment> {
    let mut tagged: Vec<(Channel, Segment)> = channels
        .into_iter()
        .flat_map(|(channel, segments)| segments.into_iter().map(move |segment| (channel, segment)))
        .collect();
    tagged.sort_by_key(|(_, segment)| segment.start_ms);

    tagged
        .into_iter()
        .enumerate()
        .map(|(index, (channel, segment))| TranscriptSegment {
            index: index as u64,
            channel,
            speaker: None,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms,
            text: segment.text,
        })
        .collect()
}

/// Writes an assembled transcript to `dir` as JSONL, named per the session
/// filename scheme, and returns the path written — the value a Phase 2 note
/// would later reference from its `source:` frontmatter field.
///
/// Never overwrites an existing file: if the composed name is already
/// taken (e.g. a same-device capture in the same millisecond, or a repeat
/// slug), an increasing numeric suffix is appended to the slug until a free
/// name is found (see [`crate::naming`]'s module docs on same-device
/// collisions). The claim is atomic — the destination is created by hard-
/// linking, which fails rather than overwriting when the name exists — so
/// two concurrent writers can never resolve to and clobber the same file.
pub fn write_raw_session(
    dir: &Path,
    captured_at: DateTime<Utc>,
    device: &DeviceId,
    slug: Option<&str>,
    segments: &[TranscriptSegment],
) -> Result<PathBuf> {
    fs::create_dir_all(dir).map_err(|source| RawSessionError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    // Serialize the whole transcript before touching the filesystem, so a
    // serialization error leaves neither a scratch file nor a claimed name
    // behind.
    let mut body = String::new();
    for segment in segments {
        let line = serde_json::to_string(segment).map_err(|source| RawSessionError::Json {
            path: dir.to_path_buf(),
            source,
        })?;
        body.push_str(&line);
        body.push('\n');
    }

    // Stage the full content in a scratch file, then atomically link it into
    // the first free name. Linking (not renaming) makes the claim exclusive —
    // it fails with `AlreadyExists` instead of clobbering an occupied name —
    // which both resolves collisions and closes the check-then-write race a
    // bare `exists()` test would leave open; a reader only ever sees the fully
    // written file, never a partial one.
    let tmp_path = stage_scratch(dir, &body)?;
    let result = link_into_free_path(&tmp_path, dir, captured_at, device, slug);
    // The linked destination now keeps the content alive; the scratch name is
    // redundant on both success and failure.
    let _ = fs::remove_file(&tmp_path);
    result
}

/// Reads back a transcript written by [`write_raw_session`], independent of
/// any database — the guarantee that makes the SQLite index disposable
/// (FOUNDING_DOC §3.6).
pub fn read_raw_session(path: &Path) -> Result<Vec<TranscriptSegment>> {
    let contents = fs::read_to_string(path).map_err(|source| RawSessionError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|source| RawSessionError::Json {
                path: path.to_path_buf(),
                source,
            })
        })
        .collect()
}

/// Whether a transcript holds nothing worth distilling: every segment's text
/// is whitespace (an empty transcript included). The single definition the
/// distill pass (which treats it as a benign skip, not a failure) and the
/// failed-session scan (which excludes it from "needs attention") share, so the
/// two agree by construction — a silent capture is never surfaced as an error
/// and never nagged about as retryable.
pub fn is_silent(segments: &[TranscriptSegment]) -> bool {
    segments
        .iter()
        .all(|segment| segment.text.trim().is_empty())
}

/// Writes `contents` to a fresh, process-unique scratch file in `dir` and
/// returns its path. On a write error the partial scratch file is removed, so
/// a failed write leaves nothing behind.
fn stage_scratch(dir: &Path, contents: &str) -> Result<PathBuf> {
    let tmp_path = unique_temp_path(dir);
    if let Err(source) = fs::write(&tmp_path, contents) {
        let _ = fs::remove_file(&tmp_path);
        return Err(RawSessionError::Io {
            path: tmp_path,
            source,
        });
    }
    Ok(tmp_path)
}

/// Hard-links the staged scratch file into the first session filename under
/// `dir` that isn't already taken, and returns that path.
///
/// `fs::hard_link` fails with `AlreadyExists` rather than overwriting, so an
/// occupied name is skipped, not clobbered — making the free-name claim
/// atomic. The disambiguator comes from [`numbered_slug`], whose number
/// survives the filename length cap, so distinct attempts always produce
/// distinct names and the loop terminates.
fn link_into_free_path(
    tmp_path: &Path,
    dir: &Path,
    captured_at: DateTime<Utc>,
    device: &DeviceId,
    slug: Option<&str>,
) -> Result<PathBuf> {
    let mut attempt: Option<u32> = None;
    loop {
        let name = match attempt {
            None => session_filename(captured_at, device, slug, "jsonl"),
            Some(n) => {
                session_filename(captured_at, device, Some(&numbered_slug(slug, n)), "jsonl")
            }
        };
        let candidate = dir.join(name);
        match fs::hard_link(tmp_path, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                attempt = Some(attempt.map_or(2, |n| n + 1));
            }
            Err(source) => {
                return Err(RawSessionError::Io {
                    path: candidate,
                    source,
                });
            }
        }
    }
}

/// Whether a persisted session transcript for this capture instant and device
/// already exists in `dir` — any `<timestamp>-<device>.jsonl`, including the
/// slugged and numbered variants [`write_raw_session`] produces on collision.
///
/// Recovery uses this to stay idempotent: the in-flight spill directory is
/// deleted only *after* its transcript lands, so a crash (or a failed removal)
/// in that window leaves a directory a later launch would otherwise transcribe
/// a second time, producing a duplicate session file and note. A `dir` that
/// can't be read (e.g. no sessions yet) reads as "not present".
pub fn session_exists(dir: &Path, captured_at: DateTime<Utc>, device: &DeviceId) -> bool {
    // `<timestamp>-<device>`: the exact stem of a bare session file, and the
    // prefix of every slugged/numbered one. The device id is fixed-length, so
    // the boundary char after the prefix is always `.` (bare) or `-` (slug).
    let prefix = session_dir_name(captured_at, device);
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.ends_with(".jsonl") {
            continue;
        }
        if let Some(rest) = name.strip_prefix(&prefix) {
            if rest.starts_with('.') || rest.starts_with('-') {
                return true;
            }
        }
    }
    false
}

/// Builds a scratch path in `dir` unique to this process and call.
fn unique_temp_path(dir: &Path) -> PathBuf {
    dir.join(format!(
        ".raw-session.{}.{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::tempdir;

    fn device() -> DeviceId {
        DeviceId::parse("k4m2xp7q").unwrap()
    }

    fn instant() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35).unwrap()
    }

    fn segment(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    #[test]
    fn is_silent_covers_empty_whitespace_and_spoken_transcripts() {
        let line = |text: &str| TranscriptSegment {
            index: 0,
            channel: Channel::You,
            speaker: None,
            start_ms: 0,
            end_ms: 500,
            text: text.to_string(),
        };

        // No segments at all, and segments holding only whitespace, are both
        // "nothing was said" — the benign skip, never a failure to retry.
        assert!(is_silent(&[]));
        assert!(is_silent(&[line("   "), line("\t\n")]));

        // One segment with real text is enough to make the session distillable.
        assert!(!is_silent(&[line("  "), line("lets sync on the budget")]));
    }

    #[test]
    fn session_exists_matches_bare_slugged_and_numbered_but_not_other_sessions() {
        let dir = tempdir().unwrap();
        let other_device = DeviceId::parse("z9y8x7w6").unwrap();

        // Nothing written yet, and a missing dir, both read as "not present".
        assert!(!session_exists(dir.path(), instant(), &device()));
        assert!(!session_exists(
            &dir.path().join("nope"),
            instant(),
            &device()
        ));

        // A slugged transcript for this instant+device counts as present.
        let name = session_filename(instant(), &device(), Some("weekly sync"), "jsonl");
        fs::write(dir.path().join(&name), "{}").unwrap();
        assert!(session_exists(dir.path(), instant(), &device()));

        // A different device (same instant) is a different session.
        assert!(!session_exists(dir.path(), instant(), &other_device));

        // A different instant (same device) is a different session.
        let later = instant() + chrono::Duration::seconds(1);
        assert!(!session_exists(dir.path(), later, &device()));
    }

    #[test]
    fn assemble_orders_by_start_ms_across_channels() {
        let you = vec![segment(0, 500, "hello"), segment(2000, 2500, "how are you")];
        let them = vec![segment(600, 1200, "hi there")];

        let transcript = assemble([(Channel::You, you), (Channel::Them, them)]);

        assert_eq!(transcript.len(), 3);
        assert_eq!(transcript[0].index, 0);
        assert_eq!(transcript[0].channel, Channel::You);
        assert_eq!(transcript[0].text, "hello");
        assert_eq!(transcript[1].index, 1);
        assert_eq!(transcript[1].channel, Channel::Them);
        assert_eq!(transcript[1].text, "hi there");
        assert_eq!(transcript[2].index, 2);
        assert_eq!(transcript[2].channel, Channel::You);
        assert_eq!(transcript[2].text, "how are you");
    }

    #[test]
    fn assemble_ties_break_by_input_order() {
        let you = vec![segment(1000, 1500, "you-first")];
        let them = vec![segment(1000, 1500, "them-second")];

        let transcript = assemble([(Channel::You, you), (Channel::Them, them)]);

        assert_eq!(transcript[0].text, "you-first");
        assert_eq!(transcript[1].text, "them-second");
    }

    #[test]
    fn assemble_handles_single_unknown_channel() {
        let segments = vec![segment(0, 500, "imported line")];

        let transcript = assemble([(Channel::Unknown, segments)]);

        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0].channel, Channel::Unknown);
        assert_eq!(transcript[0].index, 0);
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempdir().unwrap();
        let transcript = assemble([
            (Channel::You, vec![segment(0, 500, "hello")]),
            (Channel::Them, vec![segment(600, 1200, "hi there")]),
        ]);

        let path = write_raw_session(
            dir.path(),
            instant(),
            &device(),
            Some("standup"),
            &transcript,
        )
        .unwrap();
        let read_back = read_raw_session(&path).unwrap();

        assert_eq!(read_back, transcript);
    }

    #[test]
    fn written_filename_follows_the_session_scheme() {
        let dir = tempdir().unwrap();
        let transcript = assemble([(Channel::You, vec![segment(0, 500, "hello")])]);

        let path = write_raw_session(
            dir.path(),
            instant(),
            &device(),
            Some("standup"),
            &transcript,
        )
        .unwrap();

        let name = path.file_name().unwrap().to_str().unwrap();
        let parsed = crate::naming::parse_session_filename(name).expect("should parse");
        assert_eq!(parsed.device_id, "k4m2xp7q");
        assert_eq!(parsed.ext, "jsonl");
    }

    #[test]
    fn repeated_write_never_overwrites_the_first_file() {
        let dir = tempdir().unwrap();
        let first = assemble([(Channel::You, vec![segment(0, 500, "first session")])]);
        let second = assemble([(Channel::You, vec![segment(0, 500, "second session")])]);

        let first_path =
            write_raw_session(dir.path(), instant(), &device(), Some("standup"), &first).unwrap();
        let second_path =
            write_raw_session(dir.path(), instant(), &device(), Some("standup"), &second).unwrap();

        assert_ne!(first_path, second_path);
        assert_eq!(read_raw_session(&first_path).unwrap(), first);
        assert_eq!(read_raw_session(&second_path).unwrap(), second);
    }

    #[test]
    fn colliding_writes_with_an_overlong_slug_terminate_and_stay_distinct() {
        // A slug at/over the filename length cap once made collision
        // resolution loop forever, because the "-2", "-3", … disambiguator
        // was truncated back to the same name. Two writes must now still
        // resolve to two distinct, readable files.
        let dir = tempdir().unwrap();
        let long_slug = "meeting-title-".repeat(5); // well over the 40-char cap
        let first = assemble([(Channel::You, vec![segment(0, 500, "first")])]);
        let second = assemble([(Channel::You, vec![segment(0, 500, "second")])]);

        let first_path =
            write_raw_session(dir.path(), instant(), &device(), Some(&long_slug), &first).unwrap();
        let second_path =
            write_raw_session(dir.path(), instant(), &device(), Some(&long_slug), &second).unwrap();

        assert_ne!(first_path, second_path);
        assert_eq!(read_raw_session(&first_path).unwrap(), first);
        assert_eq!(read_raw_session(&second_path).unwrap(), second);
    }

    #[test]
    fn write_leaves_no_temp_file_behind() {
        let dir = tempdir().unwrap();
        let transcript = assemble([(Channel::You, vec![segment(0, 500, "hello")])]);

        write_raw_session(dir.path(), instant(), &device(), None, &transcript).unwrap();

        let stray = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().ends_with(".tmp"));
        assert!(!stray, "write should not leave a .tmp scratch file behind");
    }

    #[test]
    fn empty_transcript_round_trips_to_an_empty_vec() {
        let dir = tempdir().unwrap();

        let path = write_raw_session(dir.path(), instant(), &device(), None, &[]).unwrap();
        let read_back = read_raw_session(&path).unwrap();

        assert!(read_back.is_empty());
    }

    #[test]
    fn segment_serializes_to_the_documented_wire_shape() {
        let segment = TranscriptSegment {
            index: 0,
            channel: Channel::You,
            speaker: None,
            start_ms: 0,
            end_ms: 500,
            text: "hello".to_string(),
        };

        let json = serde_json::to_string(&segment).unwrap();
        assert_eq!(
            json,
            r#"{"index":0,"channel":"you","speaker":null,"start_ms":0,"end_ms":500,"text":"hello"}"#
        );
    }
}
