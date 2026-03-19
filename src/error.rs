use std::path::PathBuf;

/// Application-wide error type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("not found: {}", .0.display())]
    NotFound(PathBuf),

    #[error("inode not found: {0}")]
    InodeNotFound(u64),

    #[error("entry not found: parent_inode={0}, name={1}")]
    EntryNotFound(u64, String),

    #[error("inode out of range: {0} exceeds i64::MAX")]
    InodeOverflow(u64),

    #[error("WebDAV error: {status} {message}")]
    WebDav { status: u16, message: String },

    #[error("XML parse error: {0}")]
    XmlParse(String),

    #[error("sync error: {0}")]
    Sync(String),

    #[error("cache error: {0}")]
    Cache(String),

    #[error("authentication failed")]
    AuthFailed,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Whether this error is transient and worth retrying.
    pub fn is_transient(&self) -> bool {
        match self {
            Error::Io(_) => true,
            Error::Http(e) => {
                e.is_timeout() || e.is_connect() || e.status().is_some_and(|s| s.is_server_error())
            }
            Error::WebDav { status, .. } => *status >= 500,
            Error::AuthFailed => false,
            _ => false,
        }
    }

    /// Whether this error indicates a configuration problem that won't resolve on retry.
    pub fn is_config_error(&self) -> bool {
        matches!(self, Error::Config(_) | Error::AuthFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_error_is_transient() {
        let err = Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "reset",
        ));
        assert!(err.is_transient());
    }

    #[test]
    fn webdav_5xx_is_transient() {
        let err = Error::WebDav {
            status: 503,
            message: "unavailable".into(),
        };
        assert!(err.is_transient());
    }

    #[test]
    fn webdav_4xx_is_not_transient() {
        let err = Error::WebDav {
            status: 404,
            message: "not found".into(),
        };
        assert!(!err.is_transient());
    }

    #[test]
    fn auth_failed_is_not_transient() {
        let err = Error::AuthFailed;
        assert!(!err.is_transient());
    }

    #[test]
    fn config_error_is_not_transient() {
        let err = Error::Config("bad config".into());
        assert!(!err.is_transient());
    }
}
