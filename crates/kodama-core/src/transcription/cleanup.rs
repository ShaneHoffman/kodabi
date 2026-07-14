//! Glossary cleanup post-pass: a headless Claude Code call that repairs
//! glossary misrecognitions the transcription engine's own bias couldn't
//! catch — or, on Parakeet, the *only* glossary defense, since it has no
//! bias hook (see [`super::TranscriptionEngine::set_bias`]'s no-op default).
//! Runs on already-assembled [`TranscriptSegment`]s, so it is identical for
//! every engine (FOUNDING_DOC §3.4).

use crate::glossary::{Glossary, GlossaryError};
use crate::raw_session::TranscriptSegment;

/// A single headless call to Claude Code: takes a built request, returns its
/// text result.
///
/// Implemented for real by `kodama-llm`'s subprocess runner; kept as a trait
/// here so `kodama-core` stays free of any process-spawning dependency and
/// [`clean_transcript`] can be unit-tested against a mock.
pub trait HeadlessClaude {
    fn run(&self, request: &CleanupRequest) -> Result<String, CleanupRunError>;
}

/// A fully-built headless request: a system prompt pinning Claude's role and
/// the user-turn prompt carrying the glossary and transcript segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupRequest {
    pub system_prompt: String,
    pub prompt: String,
}

/// Failure invoking the headless runner.
///
/// Every variant is treated identically by [`clean_transcript`]: worth
/// logging, but never fatal — the transcript that went in comes back out
/// unchanged (FOUNDING_DOC §3.4's fail-soft requirement).
#[derive(Debug, Clone, thiserror::Error)]
pub enum CleanupRunError {
    #[error("failed to run headless Claude Code: {0}")]
    Spawn(String),
    #[error("headless Claude Code reported an error: {0}")]
    ClaudeError(String),
    #[error("headless Claude Code returned an empty result")]
    EmptyResult,
}

const SYSTEM_PROMPT: &str = "You are a narrow transcript-correction tool. You will be given a \
project glossary and a JSON array of transcript segments. Fix ONLY obvious misrecognitions of the \
listed glossary terms \u{2014} for example a homophone, mishearing, or garbled spelling that should \
clearly be one of the terms or one of its aliases. When you fix one, replace it with the term's \
exact canonical spelling from the \"term\" field, never with an alias or any other variant \
spelling. Do not rewrite, summarize, translate, paraphrase, or otherwise change any other wording, \
punctuation, or casing. Respond with ONLY a JSON array of the segments you changed, each as \
{\"index\": <number>, \"text\": \"<corrected text>\"}. If nothing needs changing, respond with an \
empty JSON array: []. No prose, no markdown fences, no explanation.";

#[derive(serde::Serialize)]
struct GlossaryEntry<'a> {
    term: &'a str,
    aliases: &'a [String],
    definition: &'a str,
}

#[derive(serde::Serialize)]
struct SegmentEntry<'a> {
    index: u64,
    text: &'a str,
}

/// Builds the headless request for `segments` against `glossary`.
///
/// Unlike [`super::glossary_bias_terms`] (which deliberately keeps only
/// canonical terms), this includes each entry's aliases and definition: the
/// post-pass is where alias normalization and misrecognition repair actually
/// happen.
pub fn build_request(segments: &[TranscriptSegment], glossary: &Glossary) -> CleanupRequest {
    let entries: Vec<GlossaryEntry> = glossary
        .terms()
        .iter()
        .map(|t| GlossaryEntry {
            term: &t.term,
            aliases: &t.aliases,
            definition: &t.definition,
        })
        .collect();
    let glossary_json = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_owned());

    let segment_entries: Vec<SegmentEntry> = segments
        .iter()
        .map(|s| SegmentEntry {
            index: s.index,
            text: &s.text,
        })
        .collect();
    let segments_json = serde_json::to_string(&segment_entries).unwrap_or_else(|_| "[]".to_owned());

    CleanupRequest {
        system_prompt: SYSTEM_PROMPT.to_owned(),
        prompt: format!("Glossary:\n{glossary_json}\n\nTranscript segments:\n{segments_json}"),
    }
}

/// One corrected segment as the model returns it.
#[derive(Debug, Clone, serde::Deserialize)]
struct Correction {
    index: u64,
    text: String,
}

/// Applies `model_output` (expected to be a JSON array of `{index, text}`)
/// onto `segments`, replacing only the `text` field of matching indices.
///
/// Any parse failure, or output that isn't a JSON array of that shape,
/// leaves `segments` completely unchanged — the fail-soft contract that lets
/// [`clean_transcript`] treat this call as infallible. `index`, `channel`,
/// `speaker`, `start_ms`, and `end_ms` are never touched.
pub fn apply_corrections(
    mut segments: Vec<TranscriptSegment>,
    model_output: &str,
) -> Vec<TranscriptSegment> {
    let Some(corrections) = parse_corrections(model_output) else {
        return segments;
    };

    for correction in corrections {
        if let Some(segment) = segments.iter_mut().find(|s| s.index == correction.index) {
            segment.text = correction.text;
        }
    }

    segments
}

fn parse_corrections(model_output: &str) -> Option<Vec<Correction>> {
    let trimmed = model_output.trim();
    if let Ok(corrections) = serde_json::from_str::<Vec<Correction>>(trimmed) {
        return Some(corrections);
    }
    // Models sometimes wrap the array in prose or a markdown fence despite
    // being told not to; fall back to the first balanced top-level `[...]`.
    let array = extract_json_array(trimmed)?;
    serde_json::from_str(array).ok()
}

/// Finds the first top-level, balanced `[...]` span in `text`, respecting
/// (and not miscounting brackets inside) quoted strings.
fn extract_json_array(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, ch) in text.char_indices().skip(start) {
        if in_string {
            match ch {
                _ if escape => escape = false,
                '\\' => escape = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Runs the glossary cleanup post-pass: builds the request, invokes `runner`,
/// and merges corrections back onto `segments`.
///
/// Never fails: an empty transcript, an empty glossary, a runner error, or
/// unparsable model output all fall back to returning `segments` unchanged.
/// An empty transcript or glossary short-circuits before calling `runner` at
/// all, so a project with no glossary never spends a token on this pass.
pub fn clean_transcript(
    runner: &dyn HeadlessClaude,
    segments: Vec<TranscriptSegment>,
    glossary: &Glossary,
) -> Vec<TranscriptSegment> {
    if segments.is_empty() || glossary.is_empty() {
        return segments;
    }

    let request = build_request(&segments, glossary);
    match runner.run(&request) {
        Ok(output) => apply_corrections(segments, &output),
        Err(_) => segments,
    }
}

/// Convenience wrapper mirroring [`super::apply_glossary_bias`]: loads
/// `project_dir`'s glossary, then runs [`clean_transcript`].
///
/// Only a malformed `_glossary.yml` surfaces as an `Err` (a user-fixable
/// mistake worth surfacing, same rationale as `apply_glossary_bias`); a
/// missing glossary file degrades to an empty glossary and the transcript
/// passes through unchanged.
pub fn clean_transcript_for_project(
    runner: &dyn HeadlessClaude,
    segments: Vec<TranscriptSegment>,
    project_dir: &std::path::Path,
) -> Result<Vec<TranscriptSegment>, GlossaryError> {
    let glossary = Glossary::load(project_dir)?;
    Ok(clean_transcript(runner, segments, &glossary))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glossary::{GlossaryTerm, OnConflict};
    use crate::transcription::Channel;

    fn term(term: &str, definition: &str, aliases: &[&str]) -> GlossaryTerm {
        GlossaryTerm {
            term: term.to_owned(),
            definition: definition.to_owned(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn glossary_with(entries: &[(&str, &str, &[&str])]) -> Glossary {
        let mut glossary = Glossary::default();
        for (t, d, a) in entries {
            glossary.upsert(term(t, d, a), OnConflict::Error).unwrap();
        }
        glossary
    }

    fn segment(index: u64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            index,
            channel: Channel::You,
            speaker: None,
            start_ms: index * 1000,
            end_ms: index * 1000 + 500,
            text: text.to_owned(),
        }
    }

    struct MockRunner(Result<String, CleanupRunError>);

    impl HeadlessClaude for MockRunner {
        fn run(&self, _request: &CleanupRequest) -> Result<String, CleanupRunError> {
            self.0.clone()
        }
    }

    struct PanicRunner;

    impl HeadlessClaude for PanicRunner {
        fn run(&self, _request: &CleanupRequest) -> Result<String, CleanupRunError> {
            panic!("runner should not be called when there is nothing to clean");
        }
    }

    #[test]
    fn empty_transcript_short_circuits_without_calling_the_runner() {
        let glossary = glossary_with(&[("OKIES", "Oklahoma migration project.", &[])]);

        let result = clean_transcript(&PanicRunner, Vec::new(), &glossary);

        assert!(result.is_empty());
    }

    #[test]
    fn empty_glossary_short_circuits_without_calling_the_runner() {
        let segments = vec![segment(0, "hello")];

        let result = clean_transcript(&PanicRunner, segments.clone(), &Glossary::default());

        assert_eq!(result, segments);
    }

    #[test]
    fn applies_only_the_corrected_segments_text() {
        let glossary = glossary_with(&[("OKIES", "Oklahoma migration project.", &["okies"])]);
        let segments = vec![
            segment(0, "the okies project"),
            segment(1, "unrelated line"),
        ];
        let runner = MockRunner(Ok(r#"[{"index":0,"text":"the OKIES project"}]"#.to_owned()));

        let result = clean_transcript(&runner, segments.clone(), &glossary);

        assert_eq!(result[0].text, "the OKIES project");
        assert_eq!(result[0].index, segments[0].index);
        assert_eq!(result[0].channel, segments[0].channel);
        assert_eq!(result[0].start_ms, segments[0].start_ms);
        assert_eq!(result[0].end_ms, segments[0].end_ms);
        assert_eq!(result[1], segments[1]);
    }

    #[test]
    fn unknown_index_in_the_response_is_ignored() {
        let glossary = glossary_with(&[("OKIES", "Oklahoma migration project.", &[])]);
        let segments = vec![segment(0, "hello")];
        let runner = MockRunner(Ok(r#"[{"index":99,"text":"ghost"}]"#.to_owned()));

        let result = clean_transcript(&runner, segments.clone(), &glossary);

        assert_eq!(result, segments);
    }

    #[test]
    fn malformed_model_output_leaves_the_transcript_unchanged() {
        let glossary = glossary_with(&[("OKIES", "Oklahoma migration project.", &[])]);
        let segments = vec![segment(0, "hello")];
        let runner = MockRunner(Ok("not json at all".to_owned()));

        let result = clean_transcript(&runner, segments.clone(), &glossary);

        assert_eq!(result, segments);
    }

    #[test]
    fn runner_error_leaves_the_transcript_unchanged() {
        let glossary = glossary_with(&[("OKIES", "Oklahoma migration project.", &[])]);
        let segments = vec![segment(0, "hello")];
        let runner = MockRunner(Err(CleanupRunError::Spawn("boom".to_owned())));

        let result = clean_transcript(&runner, segments.clone(), &glossary);

        assert_eq!(result, segments);
    }

    #[test]
    fn build_request_includes_aliases_and_definitions() {
        let glossary = glossary_with(&[(
            "OKIES",
            "Oklahoma migration project.",
            &["okies", "oh-kies"],
        )]);
        let segments = vec![segment(0, "the okies project")];

        let request = build_request(&segments, &glossary);

        assert!(request.prompt.contains("OKIES"));
        assert!(request.prompt.contains("Oklahoma migration project."));
        assert!(request.prompt.contains("oh-kies"));
        assert!(request.prompt.contains("the okies project"));
    }

    #[test]
    fn apply_corrections_recovers_from_a_surrounding_markdown_fence() {
        let segments = vec![segment(0, "broken")];
        let output = "```json\n[{\"index\":0,\"text\":\"fixed\"}]\n```";

        let result = apply_corrections(segments, output);

        assert_eq!(result[0].text, "fixed");
    }

    #[test]
    fn apply_corrections_ignores_brackets_inside_string_values() {
        let segments = vec![segment(0, "broken")];
        let output = r#"here you go: [{"index":0,"text":"see [note] for detail"}] thanks"#;

        let result = apply_corrections(segments, output);

        assert_eq!(result[0].text, "see [note] for detail");
    }
}
