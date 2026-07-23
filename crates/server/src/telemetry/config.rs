//! Telemetry configuration: metrics scrape + optional bounded OTLP export.

use std::collections::BTreeMap;
use std::fmt;

use crate::config::Profile;

/// How (or whether) to export OpenTelemetry signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtelExporterKind {
    /// No network exporter; in-process metrics/scrape only.
    None,
    /// OTLP/HTTP JSON to a collector endpoint.
    Otlp,
}

impl OtelExporterKind {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "disabled" => Ok(Self::None),
            "otlp" => Ok(Self::Otlp),
            _ => Err("MARKHAND_OTEL_EXPORTER must be none or otlp".into()),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Otlp => "otlp",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct TelemetryConfig {
    pub service_name: String,
    pub exporter: OtelExporterKind,
    pub otlp_endpoint: Option<String>,
    /// Sampling ratio in parts-per-thousand (0..=1000).
    pub sample_ratio_milli: u16,
    pub metrics_enabled: bool,
    /// Bounded export queue capacity (span/event records).
    pub export_queue_capacity: usize,
    /// When true, never dial a remote collector (tests / hermetic runs).
    pub disable_network: bool,
}

impl fmt::Debug for TelemetryConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TelemetryConfig")
            .field("service_name", &self.service_name)
            .field("exporter", &self.exporter.as_str())
            .field(
                "otlp_endpoint",
                &self.otlp_endpoint.as_ref().map(|_| "[REDACTED_ENDPOINT]"),
            )
            .field("sample_ratio_milli", &self.sample_ratio_milli)
            .field("metrics_enabled", &self.metrics_enabled)
            .field("export_queue_capacity", &self.export_queue_capacity)
            .field("disable_network", &self.disable_network)
            .finish()
    }
}

impl TelemetryConfig {
    pub const DEFAULT_EXPORT_QUEUE_CAPACITY: usize = 1024;

    pub fn disabled() -> Self {
        Self {
            service_name: "markhand".into(),
            exporter: OtelExporterKind::None,
            otlp_endpoint: None,
            sample_ratio_milli: 1000,
            metrics_enabled: true,
            export_queue_capacity: Self::DEFAULT_EXPORT_QUEUE_CAPACITY,
            disable_network: true,
        }
    }

    pub fn sample_ratio(&self) -> f64 {
        f64::from(self.sample_ratio_milli) / 1000.0
    }

    pub fn from_env_map(env: &BTreeMap<String, String>, profile: Profile) -> Result<Self, String> {
        let service_name = env
            .get("MARKHAND_OTEL_SERVICE_NAME")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "markhand".into());
        if service_name.len() > 128 || service_name.chars().any(|ch| ch.is_control()) {
            return Err("MARKHAND_OTEL_SERVICE_NAME is invalid".into());
        }

        let exporter = env
            .get("MARKHAND_OTEL_EXPORTER")
            .map(String::as_str)
            .map(OtelExporterKind::parse)
            .transpose()?
            .unwrap_or(OtelExporterKind::None);

        let otlp_endpoint = env
            .get("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT")
            .or_else(|| env.get("OTEL_EXPORTER_OTLP_ENDPOINT"))
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let sample_ratio_milli = env
            .get("MARKHAND_OTEL_TRACES_SAMPLER_ARG")
            .map(|raw| {
                let ratio = raw
                    .parse::<f64>()
                    .map_err(|_| "MARKHAND_OTEL_TRACES_SAMPLER_ARG must be a float".to_string())?;
                if !(0.0..=1.0).contains(&ratio) {
                    return Err::<u16, String>(
                        "MARKHAND_OTEL_TRACES_SAMPLER_ARG must be between 0 and 1".into(),
                    );
                }
                Ok::<u16, String>((ratio * 1000.0).round() as u16)
            })
            .transpose()?
            .unwrap_or(1000);

        let metrics_enabled = env
            .get("MARKHAND_METRICS_ENABLED")
            .or_else(|| env.get("MARKHAND_OTEL_METRICS_ENABLED"))
            .map(|raw| parse_bool(raw, "MARKHAND_METRICS_ENABLED"))
            .transpose()?
            .unwrap_or(true);

        let export_queue_capacity = env
            .get("MARKHAND_OTEL_EXPORT_QUEUE_CAPACITY")
            .map(|raw| {
                let capacity = raw.parse::<usize>().map_err(|_| {
                    "MARKHAND_OTEL_EXPORT_QUEUE_CAPACITY must be a positive integer".to_string()
                })?;
                if !(16..=65_536).contains(&capacity) {
                    return Err::<usize, String>(
                        "MARKHAND_OTEL_EXPORT_QUEUE_CAPACITY must be between 16 and 65536".into(),
                    );
                }
                Ok(capacity)
            })
            .transpose()?
            .unwrap_or(Self::DEFAULT_EXPORT_QUEUE_CAPACITY);

        let disable_network = match profile {
            Profile::Test => true,
            Profile::Dev | Profile::Prod => env
                .get("MARKHAND_OTEL_DISABLE_NETWORK")
                .map(|raw| parse_bool(raw, "MARKHAND_OTEL_DISABLE_NETWORK"))
                .transpose()?
                .unwrap_or(false),
        };

        let config = Self {
            service_name,
            exporter,
            otlp_endpoint,
            sample_ratio_milli,
            metrics_enabled,
            export_queue_capacity,
            disable_network,
        };
        config.validate(profile)?;
        Ok(config)
    }

    pub fn validate(&self, profile: Profile) -> Result<(), String> {
        match self.exporter {
            OtelExporterKind::None => {
                if profile == Profile::Prod && self.otlp_endpoint.is_some() {
                    return Err(
                        "prod MARKHAND_OTEL_EXPORTER=none cannot set OTLP endpoint (misconfig)"
                            .into(),
                    );
                }
            }
            OtelExporterKind::Otlp => {
                let Some(endpoint) = self.otlp_endpoint.as_deref() else {
                    return Err(
                        "MARKHAND_OTEL_EXPORTER=otlp requires MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT"
                            .into(),
                    );
                };
                validate_otlp_endpoint(endpoint, profile)?;
                if profile == Profile::Prod && self.disable_network {
                    return Err(
                        "prod cannot set MARKHAND_OTEL_DISABLE_NETWORK=true with otlp exporter"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }

    pub fn exporter_enabled(&self) -> bool {
        self.exporter == OtelExporterKind::Otlp && !self.disable_network
    }
}

fn parse_bool(raw: &str, name: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("{name} must be a boolean")),
    }
}

fn validate_otlp_endpoint(endpoint: &str, profile: Profile) -> Result<(), String> {
    let lower = endpoint.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT must be an absolute http(s) URL".into());
    }
    if endpoint.len() > 512 || endpoint.chars().any(|ch| ch.is_control()) {
        return Err("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT is invalid".into());
    }
    // Reject credentials / query / fragment — collectors must be plain origin(+path).
    let rest = endpoint
        .split_once("://")
        .map(|(_, r)| r)
        .unwrap_or(endpoint);
    if rest.contains('@') {
        return Err("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT must not include userinfo".into());
    }
    if rest.contains('?') {
        return Err("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT must not include a query string".into());
    }
    if rest.contains('#') {
        return Err("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT must not include a fragment".into());
    }
    if profile == Profile::Prod && lower.starts_with("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or_default();
        if host != "localhost" && host != "127.0.0.1" && host != "[::1]" && host != "::1" {
            return Err(
                "prod MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT must use https unless loopback".into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn defaults_to_disabled_exporter() {
        let config = TelemetryConfig::from_env_map(&env(&[]), Profile::Dev).unwrap();
        assert_eq!(config.exporter, OtelExporterKind::None);
        assert!(!config.exporter_enabled());
        assert!(config.metrics_enabled);
    }

    #[test]
    fn prod_otlp_requires_endpoint() {
        let err = TelemetryConfig::from_env_map(
            &env(&[("MARKHAND_OTEL_EXPORTER", "otlp")]),
            Profile::Prod,
        )
        .unwrap_err();
        assert!(err.contains("OTLP_ENDPOINT"));
    }

    #[test]
    fn test_profile_never_dials_network() {
        let config = TelemetryConfig::from_env_map(
            &env(&[
                ("MARKHAND_OTEL_EXPORTER", "otlp"),
                (
                    "MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT",
                    "http://127.0.0.1:4318",
                ),
            ]),
            Profile::Test,
        )
        .unwrap();
        assert!(config.disable_network);
        assert!(!config.exporter_enabled());
    }

    #[test]
    fn otlp_endpoint_rejects_userinfo_query_fragment() {
        for bad in [
            "http://user:pass@127.0.0.1:4318",
            "http://127.0.0.1:4318/v1/traces?x=1",
            "http://127.0.0.1:4318/v1/traces#frag",
        ] {
            let err = TelemetryConfig::from_env_map(
                &env(&[
                    ("MARKHAND_OTEL_EXPORTER", "otlp"),
                    ("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT", bad),
                ]),
                Profile::Dev,
            )
            .unwrap_err();
            assert!(
                err.contains("userinfo") || err.contains("query") || err.contains("fragment"),
                "unexpected err for {bad}: {err}"
            );
        }
    }
}
