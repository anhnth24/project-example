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
