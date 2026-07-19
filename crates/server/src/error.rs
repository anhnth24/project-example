//! Typed errors at the server boundary.

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid configuration: {0}")]
    Configuration(String),
    #[error("dependency unavailable: {0}")]
    Dependency(String),
    #[error("internal server error")]
    Internal,
}

impl AppError {
    /// Client-safe API error code; never expose the underlying dependency detail.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Configuration(_) => "configuration_invalid",
            Self::Dependency(_) => "dependency_unavailable",
            Self::Internal => "internal_error",
        }
    }
}
