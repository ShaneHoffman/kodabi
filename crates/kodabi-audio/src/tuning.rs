//! Pure env-override parsing shared by the capture-path tuning knobs
//! ([`crate::resample::ResampleParams`], [`crate::session::CaptureTuning`]) —
//! FOUNDING_DOC §3.7's resource budget, `docs/RESOURCE_BUDGET.md`. Kept
//! separate and dependency-free so the parsing itself is unit-tested without
//! mutating the real process environment, mirroring `kodabi-llm`'s
//! `apply_*_override(config, Option<String>)` pattern.

/// Parses a positive `usize` override, falling back to `current` on a blank,
/// unparsable, or non-positive value — an override that would zero out a
/// buffer/chunk size is nonsensical, not a valid "use zero".
pub(crate) fn apply_positive_usize_override(current: usize, raw: Option<String>) -> usize {
    match raw.and_then(|v| v.trim().parse::<usize>().ok()) {
        Some(v) if v > 0 => v,
        _ => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_a_valid_value() {
        assert_eq!(
            apply_positive_usize_override(256, Some("512".to_owned())),
            512
        );
    }

    #[test]
    fn falls_back_when_unset() {
        assert_eq!(apply_positive_usize_override(256, None), 256);
    }

    #[test]
    fn falls_back_on_blank() {
        assert_eq!(
            apply_positive_usize_override(256, Some("  ".to_owned())),
            256
        );
    }

    #[test]
    fn falls_back_on_garbage() {
        assert_eq!(
            apply_positive_usize_override(256, Some("nope".to_owned())),
            256
        );
    }

    #[test]
    fn falls_back_on_zero() {
        assert_eq!(
            apply_positive_usize_override(256, Some("0".to_owned())),
            256
        );
    }

    #[test]
    fn falls_back_on_negative() {
        assert_eq!(
            apply_positive_usize_override(256, Some("-4".to_owned())),
            256
        );
    }
}
