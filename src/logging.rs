//! Structured logging setup. JSON by default (production), pretty text for local
//! dev. Level is taken from `RUST_LOG` if set, otherwise `LOG_LEVEL`.

use crate::config::{Config, LogFormat};
use tracing_subscriber::EnvFilter;

pub fn init(config: &Config) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log_level.clone()));

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true);

    match config.log_format {
        LogFormat::Json => builder.json().flatten_event(true).init(),
        LogFormat::Text => builder.init(),
    }
}
