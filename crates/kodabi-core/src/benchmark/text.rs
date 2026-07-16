//! Shared text normalization for benchmark scoring: every metric needs the
//! same "what counts as a matching word" definition, so hypothesis vs.
//! reference comparisons don't silently disagree on casing or punctuation.

/// Lowercases `text` and splits it on any non-alphanumeric boundary (so
/// punctuation *and* hyphens end a token), dropping empty tokens.
///
/// Hyphens are treated as boundaries rather than preserved, so `"Tee-Track"`
/// tokenizes to `["fore", "up"]`, distinct from `"TeeTrack"` (`["teetrack"]`).
/// Reconciling that spelling difference is the glossary's job (`aliases`),
/// not this function's — see the `proper_nouns` module docs.
pub fn normalize_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Counts non-overlapping contiguous occurrences of *any* needle in
/// `variants` within `haystack` (all already-normalized token slices),
/// scanning left to right. A matched needle consumes its tokens before the
/// scan continues, so overlapping repeats (`"up up up"` searched for
/// `"up up"`) count once, not twice.
///
/// The needles are tried longest-first at each position, so variants that
/// overlap each other (a term's canonical spelling `"Acme"` alongside an
/// alias `"Acme Corp"`) can't *both* claim the same tokens — a single spoken
/// `"Acme Corp"` counts once, not once per variant. That matters because the
/// proper-noun scorer folds a term's canonical spelling and every alias into
/// one occurrence count; summing an independent per-variant count would
/// double-count wherever one variant is a subsequence of another.
pub(crate) fn count_variant_occurrences(haystack: &[String], variants: &[Vec<String>]) -> usize {
    let mut ordered: Vec<&[String]> = variants
        .iter()
        .map(Vec::as_slice)
        .filter(|v| !v.is_empty())
        .collect();
    // Longest needle first: where both a short and a long variant could match,
    // consuming the long one keeps the short one from re-counting the overlap.
    ordered.sort_by_key(|v| std::cmp::Reverse(v.len()));

    let mut count = 0;
    let mut i = 0;
    while i < haystack.len() {
        let mut consumed = 0;
        for needle in &ordered {
            let end = i + needle.len();
            if end <= haystack.len() && &haystack[i..end] == *needle {
                consumed = needle.len();
                break;
            }
        }
        if consumed > 0 {
            count += 1;
            i += consumed;
        } else {
            i += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_tokens_lowercases_and_splits_on_punctuation() {
        assert_eq!(
            normalize_tokens("Hello, World!"),
            vec!["hello".to_owned(), "world".to_owned()]
        );
    }

    #[test]
    fn normalize_tokens_splits_on_hyphens() {
        assert_eq!(
            normalize_tokens("Tee-Track"),
            vec!["fore".to_owned(), "up".to_owned()]
        );
    }

    #[test]
    fn normalize_tokens_of_empty_string_is_empty() {
        assert!(normalize_tokens("   ").is_empty());
    }

    fn tokens(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn count_variant_occurrences_counts_non_overlapping_matches() {
        let haystack = tokens(&["a", "b", "a", "b"]);
        assert_eq!(
            count_variant_occurrences(&haystack, &[tokens(&["a", "b"])]),
            2
        );
    }

    #[test]
    fn count_variant_occurrences_of_empty_needle_is_zero() {
        let haystack = tokens(&["a", "b"]);
        assert_eq!(count_variant_occurrences(&haystack, &[Vec::new()]), 0);
    }

    #[test]
    fn count_variant_occurrences_single_token_counts_every_occurrence() {
        let haystack = tokens(&["a", "a", "a"]);
        assert_eq!(count_variant_occurrences(&haystack, &[tokens(&["a"])]), 3);
    }

    #[test]
    fn overlapping_variants_do_not_double_count() {
        // "acme corp" counts once even though the bare "acme" variant would
        // also match its first token — the longer variant consumes both.
        let haystack = tokens(&["acme", "corp"]);
        let variants = vec![tokens(&["acme"]), tokens(&["acme", "corp"])];
        assert_eq!(count_variant_occurrences(&haystack, &variants), 1);
    }
}
