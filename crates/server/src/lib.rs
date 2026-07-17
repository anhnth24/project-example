//! Server boundary scaffold.
//!
//! HTTP routes, repositories and workers arrive in later phases. This crate owns no
//! database, storage or auth implementation yet.

pub mod api;
pub mod config;

/// Validates the minimum non-secret server configuration contract.
pub fn validate_configuration() -> Result<(), String> {
    config::ServerConfig::from_env().map(|_| ())
}
