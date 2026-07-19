use tracing_subscriber::EnvFilter;

use crate::config::{LogFormat, ServerConfig};

pub fn init_tracing(config: &ServerConfig) {
    let filter = EnvFilter::try_from_env("RUST_LOG")
        .or_else(|_| EnvFilter::try_new(config.log_level()))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let result = match config.log_format() {
        LogFormat::Json => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .json()
            .try_init(),
        LogFormat::Pretty => tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .pretty()
            .try_init(),
    };
    let _ = result;
}
