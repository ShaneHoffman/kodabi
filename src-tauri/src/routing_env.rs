//! Shared routing shell concerns for the two note pipelines (quick capture and
//! end-of-meeting distill): resolving the routing config from the environment,
//! and reporting the non-fatal per-project signal-load failures both pipelines
//! get back. Both resolve the routing threshold here, at the src-tauri boundary
//! — `kodabi_core::routing` takes [`RoutingConfig`] as a parameter and never
//! reads the environment itself.

use kodabi_core::routing::{ExamplesLoadFailure, GlossaryLoadFailure, RoutingConfig};

/// Environment override for the routing threshold. Its documented home is this
/// boundary; core stays env-free.
const ROUTING_THRESHOLD_ENV: &str = "KODABI_ROUTING_THRESHOLD";

/// Resolve the routing config from [`ROUTING_THRESHOLD_ENV`], falling back to
/// the default when it is unset or unparseable. An out-of-range value is
/// accepted here and clamped by `RoutingConfig::effective_threshold` at use,
/// keeping the intent visible.
pub(crate) fn routing_config_from_env() -> RoutingConfig {
    routing_config_from(std::env::var(ROUTING_THRESHOLD_ENV).ok().as_deref())
}

fn routing_config_from(raw: Option<&str>) -> RoutingConfig {
    match raw.and_then(|s| s.trim().parse::<f64>().ok()) {
        Some(threshold) => RoutingConfig { threshold },
        None => RoutingConfig::default(),
    }
}

/// One project's failure to load one routing signal. Each signal's failure type
/// is its own struct in core (they carry different error enums), but the shell
/// treats them identically — report which project, which file, and what the user
/// loses until it's fixed — so this trait is the shape
/// [`report_signal_failures`] iterates.
pub(crate) trait SignalLoadFailure {
    /// How the log line names the broken file.
    const FILE: &'static str;
    /// What routing does without it, phrased to complete "…; {DEGRADED} until
    /// fixed".
    const DEGRADED: &'static str;

    /// Slug of the project whose signal file failed to load.
    fn project(&self) -> &str;
    /// The underlying error, rendered.
    fn reason(&self) -> String;
}

impl SignalLoadFailure for GlossaryLoadFailure {
    const FILE: &'static str = "glossary";
    const DEGRADED: &'static str = "it routes on its name only";

    fn project(&self) -> &str {
        &self.project
    }

    fn reason(&self) -> String {
        self.error.to_string()
    }
}

impl SignalLoadFailure for ExamplesLoadFailure {
    const FILE: &'static str = "routing-examples log";
    const DEGRADED: &'static str = "recorded corrections there are ignored";

    fn project(&self) -> &str {
        &self.project
    }

    fn reason(&self) -> String {
        self.error.to_string()
    }
}

/// Logs each contained per-project signal-load failure. These are never
/// fallbacks: routing ran, the note landed wherever the surviving signals sent
/// it, and only the named project's contribution degraded — so this is a "fix
/// your file" report to stderr, not a UI-visible failure. `surface` names the
/// pipeline ("distill", "quick-capture").
pub(crate) fn report_signal_failures<F: SignalLoadFailure>(surface: &str, failures: &[F]) {
    for failure in failures {
        eprintln!(
            "{surface}: project \"{}\" has an unreadable {}; {} until fixed: {}",
            failure.project(),
            F::FILE,
            F::DEGRADED,
            failure.reason()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodabi_core::routing::DEFAULT_THRESHOLD;

    #[test]
    fn routing_config_reads_a_valid_override_and_falls_back_otherwise() {
        assert_eq!(routing_config_from(Some("0.4")).threshold, 0.4);
        assert_eq!(routing_config_from(Some(" 0.4 ")).threshold, 0.4);
        assert_eq!(routing_config_from(None).threshold, DEFAULT_THRESHOLD);
        assert_eq!(
            routing_config_from(Some("junk")).threshold,
            DEFAULT_THRESHOLD
        );
        assert_eq!(routing_config_from(Some("")).threshold, DEFAULT_THRESHOLD);
    }
}
