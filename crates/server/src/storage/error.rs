//! Typed storage errors (fail-closed; never stringly-typed).

use thiserror::Error;

/// Errors from object-key validation, MinIO, and Qdrant adapters.
///
/// Display messages are static and sanitized — never embed secrets, keys, or
/// caller-controlled strings.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    #[error("storage configuration is invalid")]
    ConfigInvalid,
    #[error("storage credentials are missing or empty")]
    ConfigMissingCredentials,
    /// Caller omitted org and/or authorized collection scope (fail closed).
    #[error("missing required org or collection scope")]
    MissingScope,
    #[error("object or vector point not found")]
    NotFound,
    #[error("storage precondition failed")]
    PreconditionFailed,
    #[error("invalid object key")]
    InvalidKey,
    #[error("object key does not belong to the authorized org")]
    KeyOrgMismatch,
    #[error("vector point ownership conflict")]
    OwnershipConflict,
    #[error("existing collection parameters do not match the index signature")]
    CollectionMismatch,
    #[error("storage transport error")]
    Transport,
    #[error("storage backend rejected the request")]
    Backend,
}

impl StorageError {
    /// Stable machine-facing error code (never includes secrets or keys).
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ConfigInvalid => "storage_config",
            Self::ConfigMissingCredentials => "storage_config_credentials",
            Self::MissingScope => "storage_missing_scope",
            Self::NotFound => "storage_not_found",
            Self::PreconditionFailed => "storage_precondition",
            Self::InvalidKey => "storage_invalid_key",
            Self::KeyOrgMismatch => "storage_key_org_mismatch",
            Self::OwnershipConflict => "storage_ownership_conflict",
            Self::CollectionMismatch => "storage_collection_mismatch",
            Self::Transport => "storage_transport",
            Self::Backend => "storage_backend",
        }
    }
}
