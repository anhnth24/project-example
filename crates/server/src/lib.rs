//! Server boundary scaffold.
//!
//! HTTP routes, repositories and workers arrive in later phases. This crate owns no
//! database, storage or auth implementation yet.

/// Validates the minimum non-secret server configuration contract.
pub fn validate_configuration() -> Result<(), &'static str> {
    Ok(())
}
