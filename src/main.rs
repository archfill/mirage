use clap::Parser;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

use mirage::cli::Cli;

fn build_env_filter(default_level: &str) -> EnvFilter {
    if std::env::var("RUST_LOG").is_ok() {
        return EnvFilter::from_default_env();
    }
    let level =
        mirage::config::read_log_level_from_config().unwrap_or_else(|| default_level.to_owned());
    EnvFilter::try_new(&level).unwrap_or_else(|_| EnvFilter::new(default_level))
}

fn main() -> mirage::error::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let is_daemon_start = args.windows(2).any(|w| w[0] == "daemon" && w[1] == "start");

    if is_daemon_start {
        let journald_layer = tracing_journald::layer()
            .map_err(|e| mirage::error::Error::Config(format!("journald unavailable: {e}")))?;
        tracing_subscriber::registry()
            .with(build_env_filter("info"))
            .with(journald_layer)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(build_env_filter("warn"))
            .init();
    }

    let cli = Cli::parse();
    mirage::run(&cli.command)
}
