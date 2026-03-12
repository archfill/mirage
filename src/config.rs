use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Application configuration stored in ~/.config/mirage/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server URL (e.g. https://cloud.example.com)
    pub server_url: String,
    /// Username for authentication
    pub username: String,
    /// Directory for cached file data
    pub cache_dir: PathBuf,
    /// Maximum cache size in bytes
    pub cache_limit_bytes: u64,
    /// Default mount point
    pub mount_point: PathBuf,
}
