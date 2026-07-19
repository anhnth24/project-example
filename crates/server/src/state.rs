//! Shared validated runtime state for API and worker processes.

use crate::config::{RuntimeEndpoints, ServerConfig};
use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct RuntimeState {
    pub config: ServerConfig,
    pub endpoints: RuntimeEndpoints,
}

impl RuntimeState {
    pub fn from_config(config: ServerConfig) -> Result<Self, AppError> {
        let endpoints = config
            .runtime_endpoints()
            .map_err(AppError::Configuration)?;
        Ok(Self { config, endpoints })
    }
}
