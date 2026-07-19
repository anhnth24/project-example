//! Shared validated runtime state for API and worker processes.

use crate::config::{RuntimeEndpoints, ServerConfig};
use crate::error::AppError;

#[derive(Debug, Clone)]
pub struct RuntimeState {
    config: ServerConfig,
    endpoints: RuntimeEndpoints,
}

impl RuntimeState {
    pub fn from_config(config: ServerConfig) -> Result<Self, AppError> {
        config.validate().map_err(AppError::Configuration)?;
        let endpoints = config
            .runtime_endpoints()
            .map_err(AppError::Configuration)?;
        Ok(Self { config, endpoints })
    }

    pub const fn config(&self) -> &ServerConfig {
        &self.config
    }

    pub const fn endpoints(&self) -> &RuntimeEndpoints {
        &self.endpoints
    }

    pub(crate) fn is_api_role(&self) -> bool {
        self.config.is_api_role()
    }
}
