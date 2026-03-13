use std::path::PathBuf;

use secrecy::SecretString;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Application configuration stored in ~/.config/mirage/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server URL (e.g. https://cloud.example.com)
    pub server_url: String,
    /// Username for authentication
    pub username: String,
    /// Password for authentication (prefer MIRAGE_PASSWORD env var)
    pub password: Option<String>,
    /// Directory for cached file data
    pub cache_dir: PathBuf,
    /// Maximum cache size in bytes
    pub cache_limit_bytes: u64,
    /// Default mount point
    pub mount_point: PathBuf,
}

impl Config {
    /// Resolve the password from environment variable or config field.
    ///
    /// Priority: MIRAGE_PASSWORD env var > config `password` field.
    pub fn resolve_password(&self) -> Result<SecretString> {
        if let Ok(env_pw) = std::env::var("MIRAGE_PASSWORD")
            && !env_pw.is_empty()
        {
            return Ok(SecretString::from(env_pw));
        }
        match &self.password {
            Some(pw) if !pw.is_empty() => Ok(SecretString::from(pw.clone())),
            _ => Err(Error::Config(
                "password not set: use MIRAGE_PASSWORD env var or config password field".into(),
            )),
        }
    }

    /// Get the DAV base URL for this Nextcloud server.
    pub fn dav_base_url(&self) -> String {
        let base = self.server_url.trim_end_matches('/');
        format!("{base}/remote.php/dav/files/{}/", self.username)
    }

    /// Get the DAV base path (for stripping from href in XML responses).
    pub fn dav_base_path(&self) -> String {
        format!("/remote.php/dav/files/{}/", self.username)
    }
}
