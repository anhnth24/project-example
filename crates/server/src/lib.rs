//! Markhand's Phase 1B server boundary.

pub mod api;
pub mod auth;
pub mod config;
mod config_edge;
pub mod database;
pub mod db;
pub mod error;
pub mod http;
pub mod jobs;
pub mod middleware;
pub mod routes;
pub mod services;
pub mod state;
pub mod storage;
pub mod telemetry;
pub mod workers;

/// Validates the non-secret server configuration contract.
pub fn validate_configuration() -> Result<(), String> {
    config::ServerConfig::from_env().map(|_| ())
}

/// Initialises structured logging / telemetry for a server/worker process.
///
/// Honours `RUST_LOG` (falls back to `info`). Optional OTLP export is config-gated
/// and never dials the network under the test profile. Safe to call once per process.
pub fn init_tracing() {
    let profile = std::env::var("MARKHAND_PROFILE")
        .ok()
        .as_deref()
        .map(config::Profile::parse)
        .transpose()
        .ok()
        .flatten()
        .unwrap_or(config::Profile::Dev);
    if let Err(error) = telemetry::init_from_env(profile) {
        eprintln!("telemetry init failed: {error}");
        // Fall back to local fmt-only subscriber so process can still start in dev.
        let filter = tracing_subscriber::filter::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .try_init();
        if matches!(profile, config::Profile::Prod) {
            panic!("production telemetry misconfiguration: {error}");
        }
    }
}
