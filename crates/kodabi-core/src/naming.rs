//! Filenames for captured sessions: `{timestamp}-{deviceID}[-{slug}].{ext}`.
//!
//! See `docs/FILENAME_SCHEME.md` for the full spec. The scheme guarantees
//! filenames never collide *across devices*: two devices capturing at the
//! exact same instant still produce distinct names because the device ID
//! differs — which is what makes import safe (merge, never overwrite).
//!
//! The timestamp is only millisecond-precise, so it does **not** by itself
//! disambiguate two captures on the *same* device within the same
//! millisecond; a caller writing the raw store must treat a same-device name
//! clash as a real collision to resolve (e.g. a distinct slug), not blindly
//! overwrite.
//!
//! Neither the timestamp nor the device ID ever contains `-`, so
//! [`parse_session_filename`] can losslessly split the pieces back apart.

use chrono::{DateTime, NaiveDateTime, Utc};

use crate::device::DeviceId;

const TIMESTAMP_FORMAT: &str = "%Y%m%dT%H%M%S%3fZ";
const MAX_SLUG_LEN: usize = 40;

/// Composes a session filename from a capture instant, device identity, and
/// an optional human-readable slug.
pub fn session_filename(
    captured_at: DateTime<Utc>,
    device: &DeviceId,
    slug: Option<&str>,
    ext: &str,
) -> String {
    let timestamp = captured_at.format(TIMESTAMP_FORMAT);
    match slug
        .map(|s| slugify(s, MAX_SLUG_LEN))
        .filter(|s| !s.is_empty())
    {
        Some(slug) => format!("{timestamp}-{device}-{slug}.{ext}"),
        None => format!("{timestamp}-{device}.{ext}"),
    }
}

/// Builds a slug of the form `base-n` (or a bare `n` when `base` has no
/// usable characters) whose numeric disambiguator `n` is guaranteed to
/// survive the filename scheme's length cap.
///
/// Appending `-n` to a raw slug and letting [`session_filename`] slugify the
/// result is not safe: a base at or over [`MAX_SLUG_LEN`] would be truncated
/// back to the same string for every `n`, so successive numbers collapse to
/// one filename. This reserves room for the suffix *before* slugifying, so
/// `n` always lands in the name and distinct `n` yield distinct filenames —
/// the property a collision-resolution loop needs to terminate.
pub fn numbered_slug(base: Option<&str>, n: u32) -> String {
    let suffix = n.to_string();
    // Reserve room for the "-{suffix}" the caller appends, so slugifying the
    // combined slug in `session_filename` never truncates the number away.
    let budget = MAX_SLUG_LEN.saturating_sub(suffix.len() + 1);
    match base.map(|s| slugify(s, budget)).filter(|s| !s.is_empty()) {
        Some(base) => format!("{base}-{suffix}"),
        None => suffix,
    }
}

/// The parsed components of a session filename produced by [`session_filename`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSessionName {
    pub timestamp: String,
    pub device_id: String,
    pub slug: Option<String>,
    pub ext: String,
}

/// Splits a session filename back into its timestamp, device ID, slug, and
/// extension. Returns `None` if `name` doesn't match the scheme.
pub fn parse_session_filename(name: &str) -> Option<ParsedSessionName> {
    let (stem, ext) = name.rsplit_once('.')?;
    let mut parts = stem.splitn(3, '-');
    let timestamp = parts.next()?;
    let device_id = parts.next()?;
    let slug = parts.next().map(str::to_string);

    if NaiveDateTime::parse_from_str(timestamp, TIMESTAMP_FORMAT).is_err() {
        return None;
    }
    if DeviceId::parse(device_id).is_err() {
        return None;
    }

    Some(ParsedSessionName {
        timestamp: timestamp.to_string(),
        device_id: device_id.to_string(),
        slug,
        ext: ext.to_string(),
    })
}

/// Lowercases, collapses runs of non-alphanumeric characters to a single
/// `-`, trims leading/trailing `-`, and caps the result at `max_len` bytes.
/// The output is ASCII, so `max_len` counts characters and bytes alike.
fn slugify(input: &str, max_len: usize) -> String {
    let mut slug = String::new();
    let mut last_was_dash = true; // suppresses a leading dash
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.len() > max_len {
        let mut truncated = slug[..max_len].to_string();
        while truncated.ends_with('-') {
            truncated.pop();
        }
        truncated
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn instant(millis: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 12, 14, 3, 35)
            .unwrap()
            .with_nanosecond(millis * 1_000_000)
            .unwrap()
    }

    #[test]
    fn two_devices_same_instant_produce_different_filenames() {
        let a = DeviceId::parse("k4m2xp7q").unwrap();
        let b = DeviceId::parse("q7n3ztaa").unwrap();
        let when = instant(123);

        let name_a = session_filename(when, &a, Some("briarwood golf sync"), "jsonl");
        let name_b = session_filename(when, &b, Some("briarwood golf sync"), "jsonl");

        assert_ne!(name_a, name_b);
    }

    #[test]
    fn filename_matches_scheme_and_stays_short() {
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let name = session_filename(instant(123), &device, Some("Briarwood Golf Sync!"), "jsonl");

        assert_eq!(
            name,
            "20260712T140335123Z-k4m2xp7q-briarwood-golf-sync.jsonl"
        );
        assert!(!name.contains(':'));
        assert!(
            name.len() <= 60,
            "filename too long: {name} ({} chars)",
            name.len()
        );
    }

    #[test]
    fn filename_without_slug_omits_trailing_segment() {
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let name = session_filename(instant(0), &device, None, "jsonl");

        assert_eq!(name, "20260712T140335000Z-k4m2xp7q.jsonl");
    }

    #[test]
    fn ascending_instants_sort_lexically_in_chronological_order() {
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let earlier = session_filename(instant(1), &device, None, "jsonl");
        let later = session_filename(instant(500), &device, None, "jsonl");

        let mut names = vec![later.clone(), earlier.clone()];
        names.sort();

        assert_eq!(names, vec![earlier, later]);
    }

    #[test]
    fn slugify_sanitizes_case_punctuation_and_length() {
        assert_eq!(
            slugify("Briarwood Golf Sync!", MAX_SLUG_LEN),
            "briarwood-golf-sync"
        );
        assert_eq!(
            slugify("  leading & trailing  ", MAX_SLUG_LEN),
            "leading-trailing"
        );
        assert_eq!(slugify("a___b", MAX_SLUG_LEN), "a-b");

        let long_input = "word ".repeat(20);
        let slug = slugify(&long_input, MAX_SLUG_LEN);
        assert!(slug.len() <= MAX_SLUG_LEN);
        assert!(!slug.ends_with('-'));
    }

    #[test]
    fn numbered_slug_keeps_the_number_even_for_an_overlong_base() {
        // A base already at the length cap must not swallow the suffix: each
        // n has to yield a distinct, in-cap slug or a collision loop hangs.
        let base = "a".repeat(MAX_SLUG_LEN + 5);
        let two = numbered_slug(Some(&base), 2);
        let three = numbered_slug(Some(&base), 3);

        assert!(two.ends_with("-2"), "suffix lost: {two}");
        assert!(three.ends_with("-3"), "suffix lost: {three}");
        assert_ne!(two, three);
        // The whole slug still fits, so `session_filename` won't re-truncate it.
        assert!(two.len() <= MAX_SLUG_LEN);
        assert!(three.len() <= MAX_SLUG_LEN);
    }

    #[test]
    fn numbered_slug_falls_back_to_a_bare_number_without_a_base() {
        assert_eq!(numbered_slug(None, 2), "2");
        // A base that slugifies to nothing is treated the same as no base.
        assert_eq!(numbered_slug(Some("!!!"), 7), "7");
    }

    #[test]
    fn parse_session_filename_round_trips() {
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let name = session_filename(instant(123), &device, Some("briarwood golf sync"), "jsonl");

        let parsed = parse_session_filename(&name).expect("should parse");
        assert_eq!(parsed.timestamp, "20260712T140335123Z");
        assert_eq!(parsed.device_id, "k4m2xp7q");
        assert_eq!(parsed.slug.as_deref(), Some("briarwood-golf-sync"));
        assert_eq!(parsed.ext, "jsonl");
    }

    #[test]
    fn parse_session_filename_round_trips_without_slug() {
        let device = DeviceId::parse("k4m2xp7q").unwrap();
        let name = session_filename(instant(0), &device, None, "jsonl");

        let parsed = parse_session_filename(&name).expect("should parse");
        assert_eq!(parsed.slug, None);
    }

    #[test]
    fn parse_session_filename_rejects_malformed_names() {
        assert!(parse_session_filename("not-a-session-name").is_none());
        assert!(parse_session_filename("20260712T140335123Z-short.jsonl").is_none());
        assert!(parse_session_filename("noextensionatall").is_none());
    }
}
