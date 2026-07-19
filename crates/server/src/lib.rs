//! Markhand's Phase 1B server boundary.

pub mod api;
pub mod config;
pub mod database;
pub mod db;
pub mod error;
pub mod http;
pub mod state;
pub mod telemetry;

/// Validates the non-secret server configuration contract.
pub fn validate_configuration() -> Result<(), String> {
    config::ServerConfig::from_env().map(|_| ())
}
