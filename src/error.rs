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
}

pub type Result<T> = std::result::Result<T, Error>;
