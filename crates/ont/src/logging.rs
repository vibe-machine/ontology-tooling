use anyhow::Result;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::cli::Cli;

/// Logging guard returned from `init`. Held for the lifetime of the process so
/// that buffered subscribers flush on drop.
pub struct LoggingGuard;

pub fn init(cli: &Cli) -> Result<LoggingGuard> {
    let level = match (cli.quiet, cli.verbose) {
        (true, _) => "warn",
        (false, 0) => "info",
        (false, 1) => "debug",
        (false, _) => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(false)
        .with_level(true);

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();

    Ok(LoggingGuard)
}
