pub mod backend;
pub mod cache;
pub mod cli;
pub mod config;
pub mod db;
pub mod error;
#[cfg(target_os = "linux")]
pub mod fuse;

use cli::Command;
use error::Result;

/// Run the application with the parsed CLI command.
pub fn run(command: &Command) -> Result<()> {
    match command {
        Command::Mount { mountpoint } => {
            tracing::info!(path = %mountpoint.display(), "mount requested");
        }
        Command::Unmount => {
            tracing::info!("unmount requested");
        }
        Command::Status => {
            tracing::info!("status requested");
        }
        Command::Pin { path } => {
            tracing::info!(path = %path.display(), "pin requested");
        }
        Command::Unpin { path } => {
            tracing::info!(path = %path.display(), "unpin requested");
        }
        Command::Config => {
            tracing::info!("config requested");
        }
    }
    Ok(())
}
