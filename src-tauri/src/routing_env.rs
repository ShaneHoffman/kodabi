//! Shared routing-config resolution for the two note pipelines (quick capture
//! and end-of-meeting distill). Both resolve the routing threshold from the
//! environment here, at the src-tauri boundary — `kodabi_core::routing` takes
//! [`RoutingConfig`] as a parameter and never reads the environment itself.

use kodabi_core::routing::RoutingConfig;

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
