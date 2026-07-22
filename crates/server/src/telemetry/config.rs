//! Telemetry configuration (tracing + optional OTLP exporter).

use std::collections::BTreeMap;
use std::fmt;

use crate::config::Profile;

/// How (or whether) to export OpenTelemetry signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtelExporterKind {
    /// No network exporter; in-process tracing/metrics only.
    None,
    /// OTLP/gRPC to a collector endpoint.
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
    /// When true, never dial a remote collector (tests / hermetic runs).
    pub disable_network: bool,
    /// Explicit bounded in-memory span/metric capture for tests only.
    /// Never enabled implicitly for prod/`exporter=none` general runtime.
    pub capture_in_memory: bool,
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
            .field("disable_network", &self.disable_network)
            .field("capture_in_memory", &self.capture_in_memory)
            .finish()
    }
}

impl TelemetryConfig {
    pub fn disabled() -> Self {
        Self {
            service_name: "markhand".into(),
            exporter: OtelExporterKind::None,
            otlp_endpoint: None,
            sample_ratio_milli: 1000,
            metrics_enabled: true,
            disable_network: true,
            capture_in_memory: false,
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
            .get("MARKHAND_OTEL_METRICS_ENABLED")
            .map(|raw| parse_bool(raw, "MARKHAND_OTEL_METRICS_ENABLED"))
            .transpose()?
            .unwrap_or(true);

        let disable_network = match profile {
            Profile::Test => true,
            Profile::Dev | Profile::Prod => env
                .get("MARKHAND_OTEL_DISABLE_NETWORK")
                .map(|raw| parse_bool(raw, "MARKHAND_OTEL_DISABLE_NETWORK"))
                .transpose()?
                .unwrap_or(false),
        };

        let capture_in_memory = env
            .get("MARKHAND_OTEL_CAPTURE_IN_MEMORY")
            .map(|raw| parse_bool(raw, "MARKHAND_OTEL_CAPTURE_IN_MEMORY"))
            .transpose()?
            .unwrap_or(false);

        let config = Self {
            service_name,
            exporter,
            otlp_endpoint,
            sample_ratio_milli,
            metrics_enabled,
            disable_network,
            capture_in_memory,
        };
        config.validate(profile)?;
        Ok(config)
    }

    pub fn validate(&self, profile: Profile) -> Result<(), String> {
        if self.capture_in_memory && profile == Profile::Prod {
            return Err("prod cannot enable MARKHAND_OTEL_CAPTURE_IN_MEMORY".into());
        }
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
    let url = url::Url::parse(endpoint)
        .map_err(|_| "MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT must be an absolute URL".to_string())?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT scheme must be http or https, got {other}"
            ));
        }
    }
    if url.host_str().is_none() {
        return Err("MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT must include a host".into());
    }
    if profile == Profile::Prod && url.scheme() != "https" {
        let host = url.host_str().unwrap_or_default();
        if host != "localhost" && host != "127.0.0.1" && host != "::1" {
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
                    "http://127.0.0.1:4317",
                ),
            ]),
            Profile::Test,
        )
        .unwrap();
        assert!(config.disable_network);
        assert!(!config.exporter_enabled());
    }

    #[test]
    fn prod_rejects_http_remote_endpoint() {
        let err = TelemetryConfig::from_env_map(
            &env(&[
                ("MARKHAND_OTEL_EXPORTER", "otlp"),
                (
                    "MARKHAND_OTEL_EXPORTER_OTLP_ENDPOINT",
                    "http://collector.example.com:4317",
                ),
            ]),
            Profile::Prod,
        )
        .unwrap_err();
        assert!(err.contains("https"));
    }
}
