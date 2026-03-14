// Backend abstraction layer.
//
// Defines the `Backend` trait for remote storage operations and the
// `RemoteEntry` intermediate type that bridges WebDAV responses to
// local database models.

pub mod nextcloud;
pub mod webdav_xml;

use bytes::Bytes;

use crate::db::models::{NewFileEntry, SyncState};
use crate::error::Result;

/// Intermediate representation of a remote file/directory.
///
/// Bridges the gap between raw WebDAV XML responses and the local
/// database `NewFileEntry` model.
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    /// Path relative to the sync root (e.g. "Documents/report.pdf")
    pub path: String,
    /// Whether this entry is a directory
    pub is_dir: bool,
    /// File size in bytes (0 for directories)
    pub size: u64,
    /// Last modified time as Unix timestamp
    pub mtime: i64,
    /// ETag from the server (for change detection)
    pub etag: Option<String>,
    /// Content hash (e.g. SHA-256 checksum from Nextcloud)
    pub content_hash: Option<String>,
    /// MIME content type
    pub content_type: Option<String>,
}

impl RemoteEntry {
    /// Extract the file/directory name from the path.
    pub fn name(&self) -> &str {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
    }

    /// Convert to a `NewFileEntry` suitable for database insertion.
    pub fn to_new_file_entry(&self, parent_inode: u64) -> NewFileEntry {
        let permissions = if self.is_dir { 0o755 } else { 0o644 };
        NewFileEntry {
            parent_inode,
            name: self.name().to_owned(),
            is_dir: self.is_dir,
            size: self.size,
            permissions,
            mtime: self.mtime,
            etag: self.etag.clone(),
            content_hash: self.content_hash.clone(),
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::Synced,
        }
    }
}

/// Trait abstracting remote storage operations.
///
/// All paths are relative to the user's DAV root
/// (e.g. `"Documents/report.pdf"`, not absolute URLs).
pub trait Backend: Send + Sync {
    /// List the contents of a remote directory.
    fn list_dir(&self, remote_path: &str) -> impl Future<Output = Result<Vec<RemoteEntry>>> + Send;

    /// Get metadata for a single remote entry.
    fn get_metadata(&self, remote_path: &str) -> impl Future<Output = Result<RemoteEntry>> + Send;

    /// Download a file's contents.
    fn download(&self, remote_path: &str) -> impl Future<Output = Result<Bytes>> + Send;

    /// Upload a file and return its updated metadata (including ETag).
    fn upload(
        &self,
        remote_path: &str,
        data: Bytes,
    ) -> impl Future<Output = Result<RemoteEntry>> + Send;

    /// Delete a remote file or directory.
    fn delete(&self, remote_path: &str) -> impl Future<Output = Result<()>> + Send;

    /// Move/rename a remote entry.
    fn move_entry(&self, from: &str, to: &str) -> impl Future<Output = Result<()>> + Send;

    /// Create a remote directory.
    fn create_dir(&self, remote_path: &str) -> impl Future<Output = Result<()>> + Send;

    /// Check if the backend is reachable. Default: issue a lightweight list_dir("").
    fn ping(&self) -> impl Future<Output = Result<()>> + Send {
        async { self.list_dir("").await.map(|_| ()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_entry_to_new_file_entry() {
        let entry = RemoteEntry {
            path: "Documents/report.pdf".to_owned(),
            is_dir: false,
            size: 4096,
            mtime: 1_700_000_000,
            etag: Some("abc123".to_owned()),
            content_hash: Some("sha256:deadbeef".to_owned()),
            content_type: Some("application/pdf".to_owned()),
        };

        let new_entry = entry.to_new_file_entry(42);
        assert_eq!(new_entry.parent_inode, 42);
        assert_eq!(new_entry.name, "report.pdf");
        assert!(!new_entry.is_dir);
        assert_eq!(new_entry.size, 4096);
        assert_eq!(new_entry.permissions, 0o644);
        assert_eq!(new_entry.mtime, 1_700_000_000);
        assert_eq!(new_entry.etag.as_deref(), Some("abc123"));
        assert_eq!(new_entry.content_hash.as_deref(), Some("sha256:deadbeef"));
        assert!(!new_entry.is_pinned);
        assert!(!new_entry.is_cached);
        assert_eq!(new_entry.sync_state, SyncState::Synced);
    }

    #[test]
    fn remote_entry_name_extraction() {
        let file = RemoteEntry {
            path: "a/b/c.txt".to_owned(),
            is_dir: false,
            size: 0,
            mtime: 0,
            etag: None,
            content_hash: None,
            content_type: None,
        };
        assert_eq!(file.name(), "c.txt");

        let dir = RemoteEntry {
            path: "a/b/subdir/".to_owned(),
            is_dir: true,
            size: 0,
            mtime: 0,
            etag: None,
            content_hash: None,
            content_type: None,
        };
        assert_eq!(dir.name(), "subdir");

        let root = RemoteEntry {
            path: "".to_owned(),
            is_dir: true,
            size: 0,
            mtime: 0,
            etag: None,
            content_hash: None,
            content_type: None,
        };
        assert_eq!(root.name(), "");
    }

    #[test]
    fn remote_entry_dir_permissions() {
        let entry = RemoteEntry {
            path: "Photos/".to_owned(),
            is_dir: true,
            size: 0,
            mtime: 0,
            etag: None,
            content_hash: None,
            content_type: None,
        };
        let new_entry = entry.to_new_file_entry(1);
        assert_eq!(new_entry.permissions, 0o755);
    }
}
