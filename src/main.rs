use clap::Parser;
use tracing_subscriber::EnvFilter;

use mirage::cli::Cli;

fn main() -> mirage::error::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    mirage::run(&cli.command)
}
