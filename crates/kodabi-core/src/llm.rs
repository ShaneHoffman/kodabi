//! The neutral seam every headless Claude Code pass runs through.
//!
//! Kodabi's LLM passes — the glossary cleanup post-pass
//! ([`crate::transcription::clean_transcript`]) and the end-of-meeting distill
//! ([`crate::distill`]) — share one invocation shape: a system prompt pinning
//! Claude's role, a user prompt carrying the data, and a text result back.
//! This module owns that shape. The side-effecting subprocess spawn lives in
//! `kodabi-llm`; keeping the trait here means `kodabi-core` stays free of any
//! process-spawning dependency and every pass can be unit-tested against a
//! mock runner.

/// A single headless call to Claude Code: takes a built request, returns its
/// text result.
///
/// Implemented for real by `kodabi-llm`'s subprocess runner; kept as a trait
/// here so the pure passes in this crate can be unit-tested against a mock.
pub trait HeadlessClaude {
    fn run(&self, request: &LlmRequest) -> Result<String, LlmRunError>;
}

/// A fully-built headless request: a system prompt pinning Claude's role and
/// the user-turn prompt carrying the pass's data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmRequest {
    pub system_prompt: String,
    pub prompt: String,
}

/// Failure invoking the headless runner.
///
/// What a failure *means* is each pass's call: the glossary cleanup treats
/// every variant as ignorable (fail-soft — the transcript survives unchanged,
/// FOUNDING_DOC §3.4), while the distill pass treats them all as fatal (no
/// note is better than a fabricated one).
#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmRunError {
    #[error("failed to run headless Claude Code: {0}")]
    Spawn(String),
    #[error("headless Claude Code reported an error: {0}")]
    ClaudeError(String),
    #[error("headless Claude Code returned an empty result")]
    EmptyResult,
}

/// Collects every top-level, balanced `open…close` span in `text`, in order,
/// respecting (and not miscounting delimiters inside) quoted strings.
///
/// Models sometimes wrap the JSON they were told to return bare in prose or a
/// markdown fence; each pass's parser falls back to these spans (`[`/`]` for
/// cleanup's array, `{`/`}` for distill's object) before giving up.
///
/// Two scans, because prose around the JSON can lie in either direction: the
/// first tracks quotes everywhere (so a quoted delimiter in prose, like
/// `"see [x]"`, can't open a phantom span), the second ignores quotes outside
/// any span (so a stray unbalanced prose quote can't flip string-parity and
/// hide every span after it). Spans the second scan finds that the first
/// missed are appended, so callers try the conservative reading first and
/// garbage recoveries simply fail their serde parse.
pub(crate) fn extract_balanced_spans(text: &str, open: char, close: char) -> Vec<&str> {
    let mut spans = scan_spans(text, open, close, true);
    for span in scan_spans(text, open, close, false) {
        if !spans.contains(&span) {
            spans.push(span);
        }
    }
    spans
}

/// One span-collection pass. `track_prose_quotes` controls whether a `"` seen
/// outside any span starts a string (see [`extract_balanced_spans`]); inside
/// a span quotes are always tracked — there they delimit well-formed JSON
/// strings whose contents must not be miscounted.
fn scan_spans(text: &str, open: char, close: char, track_prose_quotes: bool) -> Vec<&str> {
    let mut spans = Vec::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut start = None;

    // `char_indices` yields byte offsets, so the returned slices always fall
    // on char boundaries even when `text` contains multibyte characters (a
    // `skip`-by-byte-count approach silently overshoots past the first
    // opening delimiter).
    for (i, ch) in text.char_indices() {
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
            '"' if depth > 0 || track_prose_quotes => in_string = true,
            _ if ch == open => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            _ if ch == close && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start.take() {
                        spans.push(&text[s..i + close.len_utf8()]);
                    }
                }
            }
            _ => {}
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_a_bare_object_span() {
        let spans = extract_balanced_spans(r#"{"a":1}"#, '{', '}');

        assert_eq!(spans, vec![r#"{"a":1}"#]);
    }

    #[test]
    fn extracts_an_object_wrapped_in_a_markdown_fence() {
        let spans = extract_balanced_spans("```json\n{\"a\":1}\n```", '{', '}');

        assert_eq!(spans, vec![r#"{"a":1}"#]);
    }

    #[test]
    fn ignores_braces_inside_string_values() {
        let spans = extract_balanced_spans(r#"here: {"a":"see {b} for detail"} thanks"#, '{', '}');

        assert_eq!(spans, vec![r#"{"a":"see {b} for detail"}"#]);
    }

    #[test]
    fn ignores_an_escaped_quote_inside_a_string() {
        let spans = extract_balanced_spans(r#"{"a":"quote \" then {"}"#, '{', '}');

        assert_eq!(spans, vec![r#"{"a":"quote \" then {"}"#]);
    }

    #[test]
    fn survives_non_ascii_prose_before_the_span() {
        // A multibyte char before the opening delimiter used to make a
        // byte-count skip overshoot the span entirely.
        let spans = extract_balanced_spans("café — result: {\"a\":1}", '{', '}');

        assert_eq!(spans, vec![r#"{"a":1}"#]);
    }

    #[test]
    fn returns_every_top_level_span_in_order() {
        let spans = extract_balanced_spans("{} and then {\"a\":1}", '{', '}');

        assert_eq!(spans, vec!["{}", r#"{"a":1}"#]);
    }

    #[test]
    fn nested_delimiters_stay_inside_one_span() {
        let spans = extract_balanced_spans(r#"{"a":{"b":[1,2]}}"#, '{', '}');

        assert_eq!(spans, vec![r#"{"a":{"b":[1,2]}}"#]);
    }

    #[test]
    fn an_unclosed_span_is_not_returned() {
        let spans = extract_balanced_spans(r#"{"a":1"#, '{', '}');

        assert!(spans.is_empty());
    }

    #[test]
    fn a_stray_closer_before_any_opener_is_ignored() {
        let spans = extract_balanced_spans(r#"} noise {"a":1}"#, '{', '}');

        assert_eq!(spans, vec![r#"{"a":1}"#]);
    }

    #[test]
    fn an_unbalanced_prose_quote_does_not_hide_a_later_span() {
        // The stray quote flips string-parity for the quote-tracking scan;
        // the quote-ignoring rescan must still recover the object.
        let spans = extract_balanced_spans(r#"notes for "Budget sync: {"a":1}"#, '{', '}');

        assert!(spans.contains(&r#"{"a":1}"#), "spans: {spans:?}");
    }

    #[test]
    fn a_quoted_delimiter_in_prose_does_not_open_a_phantom_span() {
        // The conservative scan keeps this exact recovery; the rescan's
        // additions come after it, so the real object stays first.
        let spans = extract_balanced_spans(r#"see "{" then {"a":1}"#, '{', '}');

        assert_eq!(spans.first(), Some(&r#"{"a":1}"#));
    }
}
