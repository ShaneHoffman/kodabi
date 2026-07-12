//! Collision-free filenames for captured sessions: `{timestamp}-{deviceID}[-{slug}].{ext}`.
//!
//! See `docs/FILENAME_SCHEME.md` for the full spec. Two devices capturing at
//! the exact same instant still produce distinct filenames because the
//! device ID differs. Neither the timestamp nor the device ID ever contains
//! `-`, so [`parse_session_filename`] can losslessly split the pieces back
//! apart.

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
    match slug.map(slugify).filter(|s| !s.is_empty()) {
        Some(slug) => format!("{timestamp}-{device}-{slug}.{ext}"),
        None => format!("{timestamp}-{device}.{ext}"),
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
/// `-`, trims leading/trailing `-`, and caps the result at [`MAX_SLUG_LEN`].
fn slugify(input: &str) -> String {
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

    if slug.len() > MAX_SLUG_LEN {
        let mut truncated = slug[..MAX_SLUG_LEN].to_string();
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
        assert_eq!(slugify("Briarwood Golf Sync!"), "briarwood-golf-sync");
        assert_eq!(slugify("  leading & trailing  "), "leading-trailing");
        assert_eq!(slugify("a___b"), "a-b");

        let long_input = "word ".repeat(20);
        let slug = slugify(&long_input);
        assert!(slug.len() <= MAX_SLUG_LEN);
        assert!(!slug.ends_with('-'));
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
