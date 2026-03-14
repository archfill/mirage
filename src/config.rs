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
    /// Interval between background metadata syncs (seconds)
    #[serde(default = "default_sync_interval_secs")]
    pub sync_interval_secs: u64,
    /// Base retry interval in seconds for failed uploads
    #[serde(default = "default_retry_base_secs")]
    pub retry_base_secs: u64,
    /// Maximum retry interval in seconds
    #[serde(default = "default_retry_max_secs")]
    pub retry_max_secs: u64,
    /// Paths to always keep locally (auto-pin on sync)
    #[serde(default)]
    pub always_local_paths: Vec<String>,
}

fn default_sync_interval_secs() -> u64 {
    300
}

fn default_retry_base_secs() -> u64 {
    30
}

fn default_retry_max_secs() -> u64 {
    600
}

impl Config {
    /// Load configuration from `~/.config/mirage/config.toml`.
    pub fn load() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| Error::Config("could not determine config directory".into()))?;
        let path = config_dir.join("mirage").join("config.toml");
        let content = std::fs::read_to_string(&path)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
        toml::from_str(&content).map_err(|e| Error::Config(format!("failed to parse config: {e}")))
    }

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

    /// Check if a remote path should be always-local based on config.
    pub fn is_always_local(&self, remote_path: &str) -> bool {
        self.always_local_paths
            .iter()
            .any(|prefix| remote_path == prefix || remote_path.starts_with(&format!("{prefix}/")))
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

    /// Return the path to the config file.
    pub fn config_path() -> Result<std::path::PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| Error::Config("could not determine config directory".into()))?;
        Ok(config_dir.join("mirage").join("config.toml"))
    }

    /// Generate a template configuration file.
    pub fn generate_template() -> String {
        r#"# Mirage configuration

# Nextcloud server URL
server_url = "https://cloud.example.com"

# Username for authentication
username = "your-username"

# Password (prefer MIRAGE_PASSWORD environment variable instead)
# password = "your-password"

# Directory for cached file data
cache_dir = "~/.cache/mirage"

# Maximum cache size in bytes (default: 1 GB)
cache_limit_bytes = 1073741824

# Mount point for the virtual filesystem
mount_point = "~/Cloud"

# Interval between background metadata syncs in seconds (default: 300)
sync_interval_secs = 300

# Base retry interval for failed uploads in seconds (default: 30)
retry_base_secs = 30

# Maximum retry interval in seconds (default: 600)
retry_max_secs = 600

# Paths to always keep locally (glob-free prefix match)
# always_local_paths = ["Documents", "Photos/important"]
"#
        .to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_always_local_exact_match() {
        let cfg = Config {
            server_url: String::new(),
            username: String::new(),
            password: None,
            cache_dir: PathBuf::new(),
            cache_limit_bytes: 0,
            mount_point: PathBuf::new(),
            sync_interval_secs: 300,
            retry_base_secs: 30,
            retry_max_secs: 600,
            always_local_paths: vec!["Documents".into()],
        };
        assert!(cfg.is_always_local("Documents"));
        assert!(cfg.is_always_local("Documents/report.pdf"));
        assert!(!cfg.is_always_local("Photos"));
        assert!(!cfg.is_always_local("DocumentsExtra"));
    }

    #[test]
    fn is_always_local_empty() {
        let cfg = Config {
            server_url: String::new(),
            username: String::new(),
            password: None,
            cache_dir: PathBuf::new(),
            cache_limit_bytes: 0,
            mount_point: PathBuf::new(),
            sync_interval_secs: 300,
            retry_base_secs: 30,
            retry_max_secs: 600,
            always_local_paths: vec![],
        };
        assert!(!cfg.is_always_local("anything"));
    }

    #[test]
    fn deserialize_without_always_local_paths() {
        let toml_str = r#"
            server_url = "https://example.com"
            username = "user"
            cache_dir = "/tmp/cache"
            cache_limit_bytes = 1024
            mount_point = "/mnt"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert!(cfg.always_local_paths.is_empty());
    }

    #[test]
    fn deserialize_with_always_local_paths() {
        let toml_str = r#"
            server_url = "https://example.com"
            username = "user"
            cache_dir = "/tmp/cache"
            cache_limit_bytes = 1024
            mount_point = "/mnt"
            always_local_paths = ["Documents", "Photos/important"]
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.always_local_paths.len(), 2);
    }
}
