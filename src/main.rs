use clap::Parser;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

use mirage::cli::Cli;

fn main() -> mirage::error::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let is_daemon_start = args.windows(2).any(|w| w[0] == "daemon" && w[1] == "start");

    if is_daemon_start {
        let journald_layer = tracing_journald::layer()
            .map_err(|e| mirage::error::Error::Config(format!("journald unavailable: {e}")))?;
        tracing_subscriber::registry()
            .with(EnvFilter::from_default_env())
            .with(journald_layer)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    }

    let cli = Cli::parse();
    mirage::run(&cli.command)
}
