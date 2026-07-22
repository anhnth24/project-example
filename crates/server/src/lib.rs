//! Markhand's Phase 1B server boundary.

pub mod api;
pub mod auth;
pub mod config;
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

/// Initialises structured logging for a server/worker process.
///
/// Honours `RUST_LOG` (falls back to `info`). Safe to call once per process;
/// a second call is a no-op because the global subscriber is already set.
pub fn init_tracing() {
    use tracing_subscriber::filter::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
}
