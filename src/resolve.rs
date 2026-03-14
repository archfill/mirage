use std::path::Path;
use std::sync::Arc;

use bytes::Bytes;

use crate::backend::Backend;
use crate::db::Database;
use crate::db::models::SyncState;
use crate::error::{Error, Result};

/// Conflict resolution strategies.
#[derive(Debug, Clone, Copy)]
pub enum Strategy {
    /// Upload local version, overwriting remote.
    KeepLocal,
    /// Download remote version, overwriting local cache.
    KeepRemote,
    /// Rename remote to `<name>.conflict.<timestamp>.<ext>`, then upload local.
    KeepBoth,
}

/// Resolve a conflict for the given inode.
pub async fn resolve_conflict<B: Backend>(
    db: &Database,
    backend: &Arc<B>,
    cache_dir: &Path,
    inode: u64,
    remote_path: &str,
    strategy: Strategy,
) -> Result<()> {
    match strategy {
        Strategy::KeepLocal => {
            let cache_file = cache_dir.join(inode.to_string());
            let data = std::fs::read(&cache_file).map_err(Error::Io)?;
            let remote_entry = backend.upload(remote_path, Bytes::from(data)).await?;
            let entry = db.get_by_inode(inode)?;
            let updated = crate::db::models::NewFileEntry {
                parent_inode: entry.parent_inode,
                name: entry.name,
                is_dir: entry.is_dir,
                size: entry.size,
                permissions: entry.permissions,
                mtime: entry.mtime,
                etag: remote_entry.etag,
                content_hash: remote_entry.content_hash,
                is_pinned: entry.is_pinned,
                is_cached: entry.is_cached,
                sync_state: SyncState::Synced,
            };
            db.update_metadata(inode, &updated)?;
        }
        Strategy::KeepRemote => {
            let data = backend.download(remote_path).await?;
            let cache_file = cache_dir.join(inode.to_string());
            std::fs::write(&cache_file, &data).map_err(Error::Io)?;
            let remote_entry = backend.get_metadata(remote_path).await?;
            let entry = db.get_by_inode(inode)?;
            let updated = crate::db::models::NewFileEntry {
                parent_inode: entry.parent_inode,
                name: entry.name,
                is_dir: entry.is_dir,
                size: remote_entry.size,
                permissions: entry.permissions,
                mtime: remote_entry.mtime,
                etag: remote_entry.etag,
                content_hash: remote_entry.content_hash,
                is_pinned: entry.is_pinned,
                is_cached: true,
                sync_state: SyncState::Synced,
            };
            db.update_metadata(inode, &updated)?;
        }
        Strategy::KeepBoth => {
            let conflict_path = build_conflict_path(remote_path);
            backend.move_entry(remote_path, &conflict_path).await?;

            // Upload local version to original path
            let cache_file = cache_dir.join(inode.to_string());
            let data = std::fs::read(&cache_file).map_err(Error::Io)?;
            let remote_entry = backend.upload(remote_path, Bytes::from(data)).await?;
            let entry = db.get_by_inode(inode)?;
            let updated = crate::db::models::NewFileEntry {
                parent_inode: entry.parent_inode,
                name: entry.name,
                is_dir: entry.is_dir,
                size: entry.size,
                permissions: entry.permissions,
                mtime: entry.mtime,
                etag: remote_entry.etag,
                content_hash: remote_entry.content_hash,
                is_pinned: entry.is_pinned,
                is_cached: entry.is_cached,
                sync_state: SyncState::Synced,
            };
            db.update_metadata(inode, &updated)?;
        }
    }
    Ok(())
}

/// Build a conflict path like `path/name.conflict.20260314T221500.ext`
fn build_conflict_path(remote_path: &str) -> String {
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%S");
    if let Some(dot_pos) = remote_path.rfind('.') {
        let (base, ext) = remote_path.split_at(dot_pos);
        format!("{base}.conflict.{now}{ext}")
    } else {
        format!("{remote_path}.conflict.{now}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_path_with_extension() {
        let path = build_conflict_path("docs/report.pdf");
        assert!(path.starts_with("docs/report.conflict."));
        assert!(path.ends_with(".pdf"));
    }

    #[test]
    fn conflict_path_without_extension() {
        let path = build_conflict_path("docs/Makefile");
        assert!(path.starts_with("docs/Makefile.conflict."));
    }
}
