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
    /// Connect timeout in seconds
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,
    /// Request timeout in seconds
    #[serde(default = "default_request_timeout_secs")]
    pub request_timeout_secs: u64,
    /// Path to .mirageignore file
    #[serde(default = "default_ignore_file")]
    pub ignore_file: Option<PathBuf>,
    /// Remote folder to sync (e.g. "MirageTest" to only sync that folder)
    #[serde(default)]
    pub remote_base_path: Option<String>,
    /// Log level override (e.g. "debug", "info", "warn", "error")
    #[serde(default)]
    pub log_level: Option<String>,
}

fn default_connect_timeout_secs() -> u64 {
    10
}

fn default_request_timeout_secs() -> u64 {
    60
}

fn default_ignore_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("mirage").join(".mirageignore"))
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
        match &self.remote_base_path {
            Some(rp) => {
                let rp = rp.trim_matches('/');
                format!("{base}/remote.php/dav/files/{}/{rp}/", self.username)
            }
            None => format!("{base}/remote.php/dav/files/{}/", self.username),
        }
    }

    /// Get the DAV base path (for stripping from href in XML responses).
    pub fn dav_base_path(&self) -> String {
        match &self.remote_base_path {
            Some(rp) => {
                let rp = rp.trim_matches('/');
                format!("/remote.php/dav/files/{}/{rp}/", self.username)
            }
            None => format!("/remote.php/dav/files/{}/", self.username),
        }
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

# Connect timeout in seconds (default: 10)
connect_timeout_secs = 10

# Request timeout in seconds (default: 60)
request_timeout_secs = 60

# Path to ignore patterns file (default: ~/.config/mirage/.mirageignore)
# ignore_file = "~/.config/mirage/.mirageignore"

# Remote folder to sync (omit to sync entire account)
# remote_base_path = "MirageTest"

# Log level override (e.g. "debug", "info", "warn", "error")
# log_level = "info"
"#
        .to_owned()
    }
}

/// Read only the log_level field from config.toml, returning None on any error.
pub fn read_log_level_from_config() -> Option<String> {
    let config_dir = dirs::config_dir()?;
    let path = config_dir.join("mirage").join("config.toml");
    let content = std::fs::read_to_string(path).ok()?;

    #[derive(Deserialize)]
    struct Partial {
        log_level: Option<String>,
    }

    toml::from_str::<Partial>(&content).ok()?.log_level
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
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            ignore_file: None,
            remote_base_path: None,
            log_level: None,
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
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            ignore_file: None,
            remote_base_path: None,
            log_level: None,
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

    #[test]
    fn dav_base_url_without_remote_base() {
        let cfg = Config {
            server_url: "https://cloud.example.com".into(),
            username: "user".into(),
            password: None,
            cache_dir: PathBuf::new(),
            cache_limit_bytes: 0,
            mount_point: PathBuf::new(),
            sync_interval_secs: 300,
            retry_base_secs: 30,
            retry_max_secs: 600,
            always_local_paths: vec![],
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            ignore_file: None,
            remote_base_path: None,
            log_level: None,
        };
        assert_eq!(
            cfg.dav_base_url(),
            "https://cloud.example.com/remote.php/dav/files/user/"
        );
        assert_eq!(cfg.dav_base_path(), "/remote.php/dav/files/user/");
    }

    #[test]
    fn dav_base_url_with_remote_base() {
        let cfg = Config {
            server_url: "https://cloud.example.com".into(),
            username: "user".into(),
            password: None,
            cache_dir: PathBuf::new(),
            cache_limit_bytes: 0,
            mount_point: PathBuf::new(),
            sync_interval_secs: 300,
            retry_base_secs: 30,
            retry_max_secs: 600,
            always_local_paths: vec![],
            connect_timeout_secs: 10,
            request_timeout_secs: 60,
            ignore_file: None,
            remote_base_path: Some("MirageTest".into()),
            log_level: None,
        };
        assert_eq!(
            cfg.dav_base_url(),
            "https://cloud.example.com/remote.php/dav/files/user/MirageTest/"
        );
        assert_eq!(
            cfg.dav_base_path(),
            "/remote.php/dav/files/user/MirageTest/"
        );
    }

    #[test]
    fn deserialize_with_remote_base_path() {
        let toml_str = r#"
            server_url = "https://example.com"
            username = "user"
            cache_dir = "/tmp/cache"
            cache_limit_bytes = 1024
            mount_point = "/mnt"
            remote_base_path = "Sync"
        "#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.remote_base_path.as_deref(), Some("Sync"));
    }
}
