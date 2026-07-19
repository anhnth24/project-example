//! Shared repository / pool error types.

use deadpool_postgres::PoolError;
use thiserror::Error;
use tokio_postgres::Error as PgError;

/// Errors from pool checkout, transactions, and tenant-scoped queries.
#[derive(Debug, Error)]
pub enum DbError {
    #[error("database pool error")]
    Pool(#[from] PoolError),
    #[error("database query error")]
    Query(#[from] PgError),
    #[error("database configuration error: {0}")]
    Config(String),
    #[error("row not found")]
    NotFound,
    #[error("illegal document state transition from {from} to {to}")]
    IllegalTransition { from: String, to: String },
    #[error("stale document state: expected {expected}, observed {observed}")]
    StaleState { expected: String, observed: String },
}

impl DbError {
    /// Stable machine-facing error code (never includes row content).
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Pool(_) => "db_pool",
            Self::Query(_) => "db_query",
            Self::Config(_) => "db_config",
            Self::NotFound => "db_not_found",
            Self::IllegalTransition { .. } => "db_illegal_transition",
            Self::StaleState { .. } => "db_stale_state",
        }
    }
}
