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
        /// Recursively pin all children
        #[arg(short, long)]
        recursive: bool,
    },
    /// Revert a file or directory to on-demand mode
    Unpin {
        /// Path to unpin
        path: PathBuf,
        /// Recursively unpin all children
        #[arg(short, long)]
        recursive: bool,
    },
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// List files in conflict state
    Conflicts,
    /// Resolve a conflicted file
    Resolve {
        /// Path to the conflicted file
        path: PathBuf,
        #[command(subcommand)]
        strategy: ResolveStrategy,
    },
    /// Manage the mirage daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
    /// Launch the system tray application
    Tray,
    /// Open the activity window (GUI)
    Gui,
    /// Open settings window (GUI)
    Settings,
    /// Show mirage daemon logs
    Logs {
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: u32,
    },
    /// Interactive setup: test connection and store credentials in system keyring
    Setup,
}

#[derive(Debug, Subcommand)]
pub enum ResolveStrategy {
    /// Keep local version, overwriting remote
    KeepLocal,
    /// Keep remote version, overwriting local cache
    KeepRemote,
    /// Keep both: rename remote with conflict suffix, upload local
    KeepBoth,
}

#[derive(Debug, Subcommand)]
pub enum DaemonAction {
    /// Start mirage daemon (foreground, intended for systemd)
    Start,
    /// Stop the running mirage instance
    Stop,
    /// Check if mirage is running
    Status,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Show all configuration values
    List,
    /// Show a specific configuration value
    Get {
        /// Configuration key name
        key: String,
    },
    /// Update a specific configuration value
    Set {
        /// Configuration key name
        key: String,
        /// New value
        value: String,
    },
    /// Generate a template config file
    Init {
        /// Overwrite existing config
        #[arg(long)]
        force: bool,
    },
    /// Show the config file path
    Path,
}
