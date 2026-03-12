use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Mirage - Cloud file sync client with FUSE virtual filesystem
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Mount the virtual filesystem
    Mount {
        /// Path to mount the filesystem at
        mountpoint: PathBuf,
    },
    /// Unmount the virtual filesystem
    Unmount,
    /// Show sync state and cache usage
    Status,
    /// Mark a file or directory as always local
    Pin {
        /// Path to pin
        path: PathBuf,
    },
    /// Revert a file or directory to on-demand mode
    Unpin {
        /// Path to unpin
        path: PathBuf,
    },
    /// Configure server URL, auth, cache limit, etc.
    Config,
}
