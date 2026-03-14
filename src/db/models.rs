use std::fmt;
use std::str::FromStr;

/// Synchronization state of a file entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    Synced,
    PendingDownload,
    PendingUpload,
    Conflict,
}

impl fmt::Display for SyncState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncState::Synced => write!(f, "synced"),
            SyncState::PendingDownload => write!(f, "pending_download"),
            SyncState::PendingUpload => write!(f, "pending_upload"),
            SyncState::Conflict => write!(f, "conflict"),
        }
    }
}

impl FromStr for SyncState {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "synced" => Ok(SyncState::Synced),
            "pending_download" => Ok(SyncState::PendingDownload),
            "pending_upload" => Ok(SyncState::PendingUpload),
            "conflict" => Ok(SyncState::Conflict),
            other => Err(format!("unknown sync state: {other}")),
        }
    }
}

/// A file entry as stored in the database (all columns).
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub inode: u64,
    pub parent_inode: u64,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub permissions: u32,
    pub mtime: i64,
    pub etag: Option<String>,
    pub content_hash: Option<String>,
    pub is_pinned: bool,
    pub is_cached: bool,
    pub sync_state: SyncState,
}

/// A new file entry for insertion (inode is auto-assigned).
#[derive(Debug, Clone, PartialEq)]
pub struct NewFileEntry {
    pub parent_inode: u64,
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub permissions: u32,
    pub mtime: i64,
    pub etag: Option<String>,
    pub content_hash: Option<String>,
    pub is_pinned: bool,
    pub is_cached: bool,
    pub sync_state: SyncState,
}
