// SQLite metadata database.
//
// Stores file metadata (name, size, hash, ETag, permissions, timestamps)
// and serves as the authoritative source for readdir() and getattr()
// responses. All metadata queries are answered from this local DB,
// ensuring instant response times and offline capability.

pub mod models;

use std::path::Path;

use rusqlite::{Connection, Row, params};

use crate::error::{Error, Result};
use models::{FileEntry, NewFileEntry, SyncState};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS files (
    inode         INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_inode  INTEGER NOT NULL REFERENCES files(inode),
    name          TEXT    NOT NULL,
    is_dir        INTEGER NOT NULL DEFAULT 0,
    size          INTEGER NOT NULL DEFAULT 0,
    permissions   INTEGER NOT NULL DEFAULT 493,
    mtime         INTEGER NOT NULL,
    etag          TEXT,
    content_hash  TEXT,
    is_pinned     INTEGER NOT NULL DEFAULT 0,
    is_cached     INTEGER NOT NULL DEFAULT 0,
    sync_state    TEXT    NOT NULL DEFAULT 'synced'
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_parent_name ON files(parent_inode, name);
CREATE INDEX IF NOT EXISTS idx_sync_state ON files(sync_state);
";

const ROOT_INSERT: &str = "
INSERT OR IGNORE INTO files (inode, parent_inode, name, is_dir, size, permissions, mtime, sync_state)
VALUES (1, 1, '', 1, 0, 493, 0, 'synced');
";

/// Metadata database backed by SQLite.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        let db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    /// Open an existing database in read-only mode.
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA query_only=ON;")?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        Ok(Self { conn })
    }

    /// Open an in-memory database (for tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.initialize()?;
        Ok(db)
    }

    fn initialize(&self) -> Result<()> {
        self.conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        self.conn.execute_batch(SCHEMA)?;
        self.conn.execute_batch(ROOT_INSERT)?;
        Ok(())
    }

    // ── Helpers ───────────────────────────────────────────────────

    /// Convert a `u64` inode to `i64` for SQLite, rejecting values that exceed `i64::MAX`.
    fn to_i64(val: u64) -> Result<i64> {
        i64::try_from(val).map_err(|_| Error::InodeOverflow(val))
    }

    // ── Read operations ──────────────────────────────────────────

    /// Get a file entry by its inode.
    pub fn get_by_inode(&self, inode: u64) -> Result<FileEntry> {
        let inode_i64 = Self::to_i64(inode)?;
        let entry = self
            .conn
            .query_row(
                "SELECT * FROM files WHERE inode = ?1",
                params![inode_i64],
                row_to_entry,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Error::InodeNotFound(inode),
                other => Error::Database(other),
            })?;
        Ok(entry)
    }

    /// Look up a child entry by parent inode and name.
    pub fn lookup(&self, parent_inode: u64, name: &str) -> Result<FileEntry> {
        let parent_i64 = Self::to_i64(parent_inode)?;
        let entry = self
            .conn
            .query_row(
                "SELECT * FROM files WHERE parent_inode = ?1 AND name = ?2",
                params![parent_i64, name],
                row_to_entry,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    Error::EntryNotFound(parent_inode, name.to_owned())
                }
                other => Error::Database(other),
            })?;
        Ok(entry)
    }

    /// List all children of a directory.
    pub fn list_children(&self, parent_inode: u64) -> Result<Vec<FileEntry>> {
        let parent_i64 = Self::to_i64(parent_inode)?;
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM files WHERE parent_inode = ?1 AND inode != ?1")?;
        let entries = stmt
            .query_map(params![parent_i64], row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    // ── Write operations ─────────────────────────────────────────

    /// Insert a new file entry. Returns the assigned inode.
    pub fn insert(&self, entry: &NewFileEntry) -> Result<u64> {
        let parent_i64 = Self::to_i64(entry.parent_inode)?;
        self.conn.execute(
            "INSERT INTO files (parent_inode, name, is_dir, size, permissions, mtime, etag, content_hash, is_pinned, is_cached, sync_state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                parent_i64,
                entry.name,
                entry.is_dir as i64,
                entry.size as i64,
                entry.permissions as i64,
                entry.mtime,
                entry.etag,
                entry.content_hash,
                entry.is_pinned as i64,
                entry.is_cached as i64,
                entry.sync_state.to_string(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Update metadata for an existing entry (keeps the same inode).
    pub fn update_metadata(&self, inode: u64, entry: &NewFileEntry) -> Result<()> {
        let inode_i64 = Self::to_i64(inode)?;
        let parent_i64 = Self::to_i64(entry.parent_inode)?;
        let changed = self.conn.execute(
            "UPDATE files SET parent_inode=?1, name=?2, is_dir=?3, size=?4, permissions=?5, mtime=?6, etag=?7, content_hash=?8, is_pinned=?9, is_cached=?10, sync_state=?11
             WHERE inode=?12",
            params![
                parent_i64,
                entry.name,
                entry.is_dir as i64,
                entry.size as i64,
                entry.permissions as i64,
                entry.mtime,
                entry.etag,
                entry.content_hash,
                entry.is_pinned as i64,
                entry.is_cached as i64,
                entry.sync_state.to_string(),
                inode_i64,
            ],
        )?;
        if changed == 0 {
            return Err(Error::InodeNotFound(inode));
        }
        Ok(())
    }

    /// Update the sync state of an entry.
    pub fn update_sync_state(&self, inode: u64, state: SyncState) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE files SET sync_state = ?1 WHERE inode = ?2",
            params![state.to_string(), Self::to_i64(inode)?],
        )?;
        if changed == 0 {
            return Err(Error::InodeNotFound(inode));
        }
        Ok(())
    }

    /// Set the pinned flag on an entry.
    pub fn set_pinned(&self, inode: u64, pinned: bool) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE files SET is_pinned = ?1 WHERE inode = ?2",
            params![pinned as i64, Self::to_i64(inode)?],
        )?;
        if changed == 0 {
            return Err(Error::InodeNotFound(inode));
        }
        Ok(())
    }

    /// Recursively set the pinned flag on an entry and all its descendants.
    /// Returns the number of entries affected.
    pub fn set_pinned_recursive(&self, inode: u64, pinned: bool) -> Result<u64> {
        self.set_pinned(inode, pinned)?;
        let mut count = 1u64;
        let children = self.list_children(inode)?;
        for child in children {
            if child.is_dir {
                count += self.set_pinned_recursive(child.inode, pinned)?;
            } else {
                self.set_pinned(child.inode, pinned)?;
                count += 1;
            }
        }
        Ok(count)
    }

    /// Set the cached flag on an entry.
    pub fn set_cached(&self, inode: u64, cached: bool) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE files SET is_cached = ?1 WHERE inode = ?2",
            params![cached as i64, Self::to_i64(inode)?],
        )?;
        if changed == 0 {
            return Err(Error::InodeNotFound(inode));
        }
        Ok(())
    }

    /// Move/rename an entry (update parent_inode and name).
    pub fn move_entry(&self, inode: u64, new_parent: u64, new_name: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE files SET parent_inode = ?1, name = ?2 WHERE inode = ?3",
            params![Self::to_i64(new_parent)?, new_name, Self::to_i64(inode)?],
        )?;
        if changed == 0 {
            return Err(Error::InodeNotFound(inode));
        }
        Ok(())
    }

    /// Update size, mtime, and sync_state for an entry (used after write flush).
    pub fn update_file_after_write(&self, inode: u64, size: u64, mtime: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE files SET size = ?1, mtime = ?2, sync_state = ?3 WHERE inode = ?4",
            params![
                size as i64,
                mtime,
                SyncState::PendingUpload.to_string(),
                Self::to_i64(inode)?
            ],
        )?;
        if changed == 0 {
            return Err(Error::InodeNotFound(inode));
        }
        Ok(())
    }

    /// Delete an entry by inode.
    pub fn delete(&self, inode: u64) -> Result<()> {
        let changed = self.conn.execute(
            "DELETE FROM files WHERE inode = ?1",
            params![Self::to_i64(inode)?],
        )?;
        if changed == 0 {
            return Err(Error::InodeNotFound(inode));
        }
        Ok(())
    }

    // ── Aggregate queries ─────────────────────────────────────────

    /// Count total number of file entries (excluding root).
    pub fn count_total(&self) -> Result<u64> {
        let n: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM files WHERE inode != 1", [], |r| {
                    r.get(0)
                })?;
        Ok(n as u64)
    }

    /// Count entries that are cached locally.
    pub fn count_cached(&self) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE is_cached = 1 AND inode != 1",
            [],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    /// Count entries in a specific sync state (excluding root).
    pub fn count_by_sync_state(&self, state: SyncState) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE sync_state = ?1 AND inode != 1",
            params![state.to_string()],
            |r| r.get(0),
        )?;
        Ok(n as u64)
    }

    // ── Query operations ─────────────────────────────────────────

    /// Get all pinned entries.
    pub fn get_pinned_entries(&self) -> Result<Vec<FileEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM files WHERE is_pinned = 1")?;
        let entries = stmt
            .query_map([], row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// Get all entries with a given sync state.
    pub fn get_by_sync_state(&self, state: SyncState) -> Result<Vec<FileEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM files WHERE sync_state = ?1")?;
        let entries = stmt
            .query_map(params![state.to_string()], row_to_entry)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }
}

/// Convert a database row to a `FileEntry`.
fn row_to_entry(row: &Row<'_>) -> rusqlite::Result<FileEntry> {
    let sync_str: String = row.get("sync_state")?;
    let sync_state = sync_str.parse().unwrap_or(SyncState::Synced);

    Ok(FileEntry {
        inode: row.get::<_, i64>("inode")? as u64,
        parent_inode: row.get::<_, i64>("parent_inode")? as u64,
        name: row.get("name")?,
        is_dir: row.get::<_, i64>("is_dir")? != 0,
        size: row.get::<_, i64>("size")? as u64,
        permissions: row.get::<_, i64>("permissions")? as u32,
        mtime: row.get("mtime")?,
        etag: row.get("etag")?,
        content_hash: row.get("content_hash")?,
        is_pinned: row.get::<_, i64>("is_pinned")? != 0,
        is_cached: row.get::<_, i64>("is_cached")? != 0,
        sync_state,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Database {
        Database::open_in_memory().expect("failed to open in-memory db")
    }

    fn sample_entry(parent: u64, name: &str, is_dir: bool) -> NewFileEntry {
        NewFileEntry {
            parent_inode: parent,
            name: name.to_owned(),
            is_dir,
            size: 1024,
            permissions: 0o644,
            mtime: 1_700_000_000,
            etag: Some("abc123".to_owned()),
            content_hash: Some("sha256:deadbeef".to_owned()),
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::Synced,
        }
    }

    #[test]
    fn root_exists_after_init() {
        let db = test_db();
        let root = db.get_by_inode(1).unwrap();
        assert_eq!(root.inode, 1);
        assert_eq!(root.parent_inode, 1);
        assert!(root.is_dir);
        assert_eq!(root.name, "");
    }

    #[test]
    fn insert_and_get_by_inode() {
        let db = test_db();
        let inode = db.insert(&sample_entry(1, "hello.txt", false)).unwrap();
        let entry = db.get_by_inode(inode).unwrap();
        assert_eq!(entry.name, "hello.txt");
        assert_eq!(entry.size, 1024);
        assert!(!entry.is_dir);
    }

    #[test]
    fn lookup_by_parent_and_name() {
        let db = test_db();
        db.insert(&sample_entry(1, "docs", true)).unwrap();
        let entry = db.lookup(1, "docs").unwrap();
        assert_eq!(entry.name, "docs");
        assert!(entry.is_dir);
    }

    #[test]
    fn lookup_not_found() {
        let db = test_db();
        let err = db.lookup(1, "nonexistent").unwrap_err();
        assert!(matches!(err, Error::EntryNotFound(1, _)));
    }

    #[test]
    fn get_by_inode_not_found() {
        let db = test_db();
        let err = db.get_by_inode(9999).unwrap_err();
        assert!(matches!(err, Error::InodeNotFound(9999)));
    }

    #[test]
    fn list_children() {
        let db = test_db();
        db.insert(&sample_entry(1, "a.txt", false)).unwrap();
        db.insert(&sample_entry(1, "b.txt", false)).unwrap();
        db.insert(&sample_entry(1, "subdir", true)).unwrap();

        let children = db.list_children(1).unwrap();
        assert_eq!(children.len(), 3);
    }

    #[test]
    fn list_children_excludes_self() {
        let db = test_db();
        // Root's parent_inode == inode == 1; it should not appear in its own children.
        let children = db.list_children(1).unwrap();
        assert!(children.is_empty());
    }

    #[test]
    fn update_metadata() {
        let db = test_db();
        let inode = db.insert(&sample_entry(1, "old.txt", false)).unwrap();

        let mut updated = sample_entry(1, "new.txt", false);
        updated.size = 2048;
        db.update_metadata(inode, &updated).unwrap();

        let entry = db.get_by_inode(inode).unwrap();
        assert_eq!(entry.name, "new.txt");
        assert_eq!(entry.size, 2048);
    }

    #[test]
    fn update_metadata_not_found() {
        let db = test_db();
        let err = db
            .update_metadata(9999, &sample_entry(1, "x", false))
            .unwrap_err();
        assert!(matches!(err, Error::InodeNotFound(9999)));
    }

    #[test]
    fn update_sync_state() {
        let db = test_db();
        let inode = db.insert(&sample_entry(1, "file.txt", false)).unwrap();

        db.update_sync_state(inode, SyncState::PendingUpload)
            .unwrap();
        let entry = db.get_by_inode(inode).unwrap();
        assert_eq!(entry.sync_state, SyncState::PendingUpload);
    }

    #[test]
    fn set_pinned() {
        let db = test_db();
        let inode = db.insert(&sample_entry(1, "important.txt", false)).unwrap();

        db.set_pinned(inode, true).unwrap();
        let entry = db.get_by_inode(inode).unwrap();
        assert!(entry.is_pinned);

        db.set_pinned(inode, false).unwrap();
        let entry = db.get_by_inode(inode).unwrap();
        assert!(!entry.is_pinned);
    }

    #[test]
    fn set_cached() {
        let db = test_db();
        let inode = db.insert(&sample_entry(1, "data.bin", false)).unwrap();

        db.set_cached(inode, true).unwrap();
        let entry = db.get_by_inode(inode).unwrap();
        assert!(entry.is_cached);
    }

    #[test]
    fn delete_entry() {
        let db = test_db();
        let inode = db.insert(&sample_entry(1, "temp.txt", false)).unwrap();
        db.delete(inode).unwrap();

        let err = db.get_by_inode(inode).unwrap_err();
        assert!(matches!(err, Error::InodeNotFound(_)));
    }

    #[test]
    fn delete_not_found() {
        let db = test_db();
        let err = db.delete(9999).unwrap_err();
        assert!(matches!(err, Error::InodeNotFound(9999)));
    }

    #[test]
    fn get_pinned_entries() {
        let db = test_db();
        let i1 = db.insert(&sample_entry(1, "a.txt", false)).unwrap();
        let i2 = db.insert(&sample_entry(1, "b.txt", false)).unwrap();
        db.insert(&sample_entry(1, "c.txt", false)).unwrap();

        db.set_pinned(i1, true).unwrap();
        db.set_pinned(i2, true).unwrap();

        let pinned = db.get_pinned_entries().unwrap();
        assert_eq!(pinned.len(), 2);
    }

    #[test]
    fn get_by_sync_state() {
        let db = test_db();
        let i1 = db.insert(&sample_entry(1, "a.txt", false)).unwrap();
        db.insert(&sample_entry(1, "b.txt", false)).unwrap();

        db.update_sync_state(i1, SyncState::PendingDownload)
            .unwrap();

        let pending = db.get_by_sync_state(SyncState::PendingDownload).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "a.txt");

        let synced = db.get_by_sync_state(SyncState::Synced).unwrap();
        // root + b.txt
        assert_eq!(synced.len(), 2);
    }

    #[test]
    fn unique_constraint_on_parent_name() {
        let db = test_db();
        db.insert(&sample_entry(1, "dup.txt", false)).unwrap();
        let err = db.insert(&sample_entry(1, "dup.txt", false)).unwrap_err();
        assert!(matches!(err, Error::Database(_)));
    }

    #[test]
    fn count_total() {
        let db = test_db();
        assert_eq!(db.count_total().unwrap(), 0);
        db.insert(&sample_entry(1, "a.txt", false)).unwrap();
        db.insert(&sample_entry(1, "b.txt", false)).unwrap();
        assert_eq!(db.count_total().unwrap(), 2);
    }

    #[test]
    fn count_cached() {
        let db = test_db();
        let i1 = db.insert(&sample_entry(1, "a.txt", false)).unwrap();
        db.insert(&sample_entry(1, "b.txt", false)).unwrap();
        assert_eq!(db.count_cached().unwrap(), 0);
        db.set_cached(i1, true).unwrap();
        assert_eq!(db.count_cached().unwrap(), 1);
    }

    #[test]
    fn count_by_sync_state() {
        let db = test_db();
        let i1 = db.insert(&sample_entry(1, "a.txt", false)).unwrap();
        db.insert(&sample_entry(1, "b.txt", false)).unwrap();
        // Both start as Synced; root is excluded
        assert_eq!(db.count_by_sync_state(SyncState::Synced).unwrap(), 2);
        db.update_sync_state(i1, SyncState::PendingDownload)
            .unwrap();
        assert_eq!(
            db.count_by_sync_state(SyncState::PendingDownload).unwrap(),
            1
        );
        assert_eq!(db.count_by_sync_state(SyncState::Synced).unwrap(), 1);
    }

    #[test]
    fn set_pinned_recursive() {
        let db = test_db();
        // Build a 3-level tree: root -> dir_a -> dir_b -> file_deep
        //                        root -> dir_a -> file_shallow
        //                        root -> file_root
        let dir_a = db.insert(&sample_entry(1, "dir_a", true)).unwrap();
        let file_shallow = db
            .insert(&sample_entry(dir_a, "file_shallow.txt", false))
            .unwrap();
        let dir_b = db.insert(&sample_entry(dir_a, "dir_b", true)).unwrap();
        let file_deep = db
            .insert(&sample_entry(dir_b, "file_deep.txt", false))
            .unwrap();
        let file_root = db.insert(&sample_entry(1, "file_root.txt", false)).unwrap();

        // Recursive pin on dir_a should pin dir_a, file_shallow, dir_b, file_deep
        let count = db.set_pinned_recursive(dir_a, true).unwrap();
        assert_eq!(count, 4);

        assert!(db.get_by_inode(dir_a).unwrap().is_pinned);
        assert!(db.get_by_inode(file_shallow).unwrap().is_pinned);
        assert!(db.get_by_inode(dir_b).unwrap().is_pinned);
        assert!(db.get_by_inode(file_deep).unwrap().is_pinned);
        // file_root should NOT be pinned
        assert!(!db.get_by_inode(file_root).unwrap().is_pinned);

        // Recursive unpin
        let count = db.set_pinned_recursive(dir_a, false).unwrap();
        assert_eq!(count, 4);
        assert!(!db.get_by_inode(dir_a).unwrap().is_pinned);
        assert!(!db.get_by_inode(file_shallow).unwrap().is_pinned);
        assert!(!db.get_by_inode(dir_b).unwrap().is_pinned);
        assert!(!db.get_by_inode(file_deep).unwrap().is_pinned);
    }

    #[test]
    fn inode_overflow_rejected() {
        let db = test_db();
        let huge: u64 = u64::MAX;
        let err = db.get_by_inode(huge).unwrap_err();
        assert!(matches!(err, Error::InodeOverflow(_)));
    }
}
