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
