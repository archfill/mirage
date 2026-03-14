// Sync engine — bridges the Backend and Database layers.
//
// Fetches remote metadata via Backend, compares with local DB state
// using the reconciler, and applies the resulting actions to the DB.

pub mod reconciler;

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;

use crate::backend::Backend;
use crate::db::Database;
use crate::db::models::SyncState;
use crate::error::{Error, Result};

use reconciler::{SyncAction, reconcile};

/// Summary of a sync operation.
#[derive(Debug, Default)]
pub struct SyncReport {
    pub added: u64,
    pub updated: u64,
    pub deleted: u64,
    pub pinned_downloads: u64,
    pub errors: Vec<SyncError>,
}

impl SyncReport {
    fn merge(&mut self, other: SyncReport) {
        self.added += other.added;
        self.updated += other.updated;
        self.deleted += other.deleted;
        self.pinned_downloads += other.pinned_downloads;
        self.errors.extend(other.errors);
    }
}

/// A per-entry error that didn't stop the overall sync.
#[derive(Debug)]
pub struct SyncError {
    pub path: String,
    pub error: Error,
}

/// Metadata sync engine.
///
/// Owns a dedicated `Database` connection (SQLite WAL allows concurrent readers).
pub struct SyncEngine<B: Backend> {
    db: Database,
    backend: Arc<B>,
    cache_dir: PathBuf,
    always_local_paths: Vec<String>,
}

impl<B: Backend> SyncEngine<B> {
    pub fn new(
        db: Database,
        backend: Arc<B>,
        cache_dir: PathBuf,
        always_local_paths: Vec<String>,
    ) -> Self {
        Self {
            db,
            backend,
            cache_dir,
            always_local_paths,
        }
    }

    /// Sync a single directory (non-recursive).
    pub async fn sync_dir(&self, inode: u64) -> Result<SyncReport> {
        let remote_path = self.resolve_remote_path(inode)?;
        self.sync_dir_impl(inode, &remote_path).await
    }

    /// Full recursive sync from the root directory.
    pub async fn full_sync(&self) -> Result<SyncReport> {
        let mut report = self.sync_dir_recursive(1, "").await?;
        match self.download_pinned().await {
            Ok(n) => report.pinned_downloads = n,
            Err(e) => tracing::error!(error = %e, "pinned download failed"),
        }
        Ok(report)
    }

    /// Download pinned files that are not yet cached.
    async fn download_pinned(&self) -> Result<u64> {
        let pinned = self.db.get_pinned_entries()?;
        let mut count = 0u64;
        for entry in pinned {
            if entry.is_cached || entry.is_dir {
                continue;
            }
            let remote_path = self.resolve_remote_path(entry.inode)?;
            match self.backend.download(&remote_path).await {
                Ok(data) => {
                    let cache_file = self.cache_dir.join(entry.inode.to_string());
                    if let Err(e) = std::fs::write(&cache_file, &data) {
                        tracing::error!(inode = entry.inode, error = %e, "failed to cache pinned file");
                        continue;
                    }
                    if let Err(e) = self.db.set_cached(entry.inode, true) {
                        tracing::error!(inode = entry.inode, error = %e, "failed to set cached flag");
                        continue;
                    }
                    if entry.sync_state == SyncState::PendingDownload {
                        let _ = self.db.update_sync_state(entry.inode, SyncState::Synced);
                    }
                    tracing::info!(
                        inode = entry.inode,
                        path = remote_path,
                        "pinned file downloaded"
                    );
                    count += 1;
                }
                Err(e) => {
                    tracing::error!(inode = entry.inode, path = remote_path, error = %e, "failed to download pinned file");
                }
            }
        }
        Ok(count)
    }

    /// Resolve the remote path for an inode by walking the parent chain.
    fn resolve_remote_path(&self, inode: u64) -> Result<String> {
        if inode == 1 {
            return Ok(String::new());
        }
        let mut parts = Vec::new();
        let mut current = inode;
        loop {
            let entry = self.db.get_by_inode(current)?;
            if current == 1 || entry.parent_inode == current {
                break;
            }
            parts.push(entry.name);
            current = entry.parent_inode;
        }
        parts.reverse();
        Ok(parts.join("/"))
    }

    /// Recursive sync implementation.
    fn sync_dir_recursive<'a>(
        &'a self,
        inode: u64,
        remote_path: &'a str,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<SyncReport>> + 'a>> {
        Box::pin(async move {
            let mut report = self.sync_dir_impl(inode, remote_path).await?;

            // Recurse into subdirectories
            let children = self.db.list_children(inode)?;
            for child in children {
                if child.is_dir {
                    let child_path = if remote_path.is_empty() {
                        child.name.clone()
                    } else {
                        format!("{}/{}", remote_path, child.name)
                    };
                    match self.sync_dir_recursive(child.inode, &child_path).await {
                        Ok(sub_report) => report.merge(sub_report),
                        Err(e) => {
                            report.errors.push(SyncError {
                                path: child_path,
                                error: e,
                            });
                        }
                    }
                }
            }

            Ok(report)
        })
    }

    /// Core sync logic for a single directory.
    async fn sync_dir_impl(&self, inode: u64, remote_path: &str) -> Result<SyncReport> {
        let remote_entries = self.backend.list_dir(remote_path).await?;
        let local_entries = self.db.list_children(inode)?;

        let actions = reconcile(inode, &remote_entries, &local_entries);
        let mut report = SyncReport::default();

        for action in actions {
            match self.apply_action(&action, remote_path) {
                Ok(()) => match &action {
                    SyncAction::Insert(_) => report.added += 1,
                    SyncAction::Update { .. } => report.updated += 1,
                    SyncAction::Delete { .. } => report.deleted += 1,
                },
                Err(e) => {
                    let path = action_path(&action, remote_path);
                    report.errors.push(SyncError { path, error: e });
                }
            }
        }

        Ok(report)
    }

    fn is_always_local(&self, remote_path: &str) -> bool {
        self.always_local_paths
            .iter()
            .any(|prefix| remote_path == prefix || remote_path.starts_with(&format!("{prefix}/")))
    }

    /// Apply a single sync action to the database.
    fn apply_action(&self, action: &SyncAction, parent_remote_path: &str) -> Result<()> {
        match action {
            SyncAction::Insert(entry) => {
                let child_path = if parent_remote_path.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", parent_remote_path, entry.name)
                };
                if self.is_always_local(&child_path) && !entry.is_pinned {
                    let mut pinned_entry = entry.clone();
                    pinned_entry.is_pinned = true;
                    self.db.insert(&pinned_entry)?;
                } else {
                    self.db.insert(entry)?;
                }
            }
            SyncAction::Update { inode, entry } => {
                // Check for conflict: reconciler already set Conflict state
                if entry.sync_state == SyncState::Conflict {
                    tracing::warn!(
                        inode = inode,
                        name = %entry.name,
                        "conflict detected: local PendingUpload and remote changed"
                    );
                }
                self.db.update_metadata(*inode, entry)?;
            }
            SyncAction::Delete { inode } => {
                self.db.delete(*inode)?;
            }
        }
        Ok(())
    }
}

/// Extract a path string from an action for error reporting.
fn action_path(action: &SyncAction, parent_path: &str) -> String {
    let name = match action {
        SyncAction::Insert(e) => &e.name,
        SyncAction::Update { entry, .. } => &entry.name,
        SyncAction::Delete { inode } => return format!("{parent_path}/inode:{inode}"),
    };
    if parent_path.is_empty() {
        name.clone()
    } else {
        format!("{parent_path}/{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::RemoteEntry;
    use bytes::Bytes;
    use std::sync::Mutex;

    // ── MockBackend ───────────────────────────────────────────────

    struct MockBackend {
        dirs: Mutex<std::collections::HashMap<String, Vec<RemoteEntry>>>,
        error_paths: Mutex<Vec<String>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                dirs: Mutex::new(std::collections::HashMap::new()),
                error_paths: Mutex::new(Vec::new()),
            }
        }

        fn add_dir(&self, path: &str, entries: Vec<RemoteEntry>) {
            self.dirs.lock().unwrap().insert(path.to_owned(), entries);
        }

        fn add_error_path(&self, path: &str) {
            self.error_paths.lock().unwrap().push(path.to_owned());
        }
    }

    impl Backend for MockBackend {
        async fn list_dir(&self, remote_path: &str) -> Result<Vec<RemoteEntry>> {
            if self
                .error_paths
                .lock()
                .unwrap()
                .contains(&remote_path.to_owned())
            {
                return Err(Error::Sync(format!("mock error for {remote_path}")));
            }
            Ok(self
                .dirs
                .lock()
                .unwrap()
                .get(remote_path)
                .cloned()
                .unwrap_or_default())
        }

        async fn get_metadata(&self, _remote_path: &str) -> Result<RemoteEntry> {
            unimplemented!("not needed for sync tests")
        }

        async fn download(&self, _remote_path: &str) -> Result<Bytes> {
            Ok(Bytes::new())
        }

        async fn upload(&self, _remote_path: &str, _data: Bytes) -> Result<RemoteEntry> {
            unimplemented!("not needed for sync tests")
        }

        async fn delete(&self, _remote_path: &str) -> Result<()> {
            unimplemented!("not needed for sync tests")
        }

        async fn move_entry(&self, _from: &str, _to: &str) -> Result<()> {
            unimplemented!("not needed for sync tests")
        }

        async fn create_dir(&self, _remote_path: &str) -> Result<()> {
            unimplemented!("not needed for sync tests")
        }
    }

    fn make_remote(name: &str, is_dir: bool, etag: &str) -> RemoteEntry {
        RemoteEntry {
            path: name.to_owned(),
            is_dir,
            size: if is_dir { 0 } else { 100 },
            mtime: 1_000_000,
            etag: Some(etag.to_owned()),
            content_hash: None,
            content_type: None,
        }
    }

    fn test_engine(mock: Arc<MockBackend>) -> SyncEngine<MockBackend> {
        let db = Database::open_in_memory().expect("failed to open in-memory db");
        SyncEngine::new(db, mock, std::env::temp_dir(), vec![])
    }

    // ── Integration tests ─────────────────────────────────────────

    // 1. Empty remote full_sync → DB has only root
    #[tokio::test]
    async fn full_sync_empty_remote() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir("", vec![]);
        let engine = test_engine(mock);

        let report = engine.full_sync().await.unwrap();
        assert_eq!(report.added, 0);
        assert_eq!(report.updated, 0);
        assert_eq!(report.deleted, 0);

        let children = engine.db.list_children(1).unwrap();
        assert!(children.is_empty());
    }

    // 2. Flat directory sync → all entries in DB with correct parent
    #[tokio::test]
    async fn full_sync_flat_directory() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir(
            "",
            vec![
                make_remote("file1.txt", false, "e1"),
                make_remote("file2.txt", false, "e2"),
                make_remote("subdir", true, "e3"),
            ],
        );
        // subdir is empty
        mock.add_dir("subdir", vec![]);

        let engine = test_engine(mock);
        let report = engine.full_sync().await.unwrap();

        assert_eq!(report.added, 3);
        assert_eq!(report.updated, 0);
        assert_eq!(report.deleted, 0);

        let children = engine.db.list_children(1).unwrap();
        assert_eq!(children.len(), 3);
        for child in &children {
            assert_eq!(child.parent_inode, 1);
        }
    }

    // 3. Nested directory recursive sync → correct inode hierarchy
    #[tokio::test]
    async fn full_sync_nested_directories() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir("", vec![make_remote("docs", true, "e1")]);
        mock.add_dir(
            "docs",
            vec![
                make_remote("readme.md", false, "e2"),
                make_remote("images", true, "e3"),
            ],
        );
        mock.add_dir("docs/images", vec![make_remote("logo.png", false, "e4")]);

        let engine = test_engine(mock);
        let report = engine.full_sync().await.unwrap();

        assert_eq!(report.added, 4); // docs, readme.md, images, logo.png

        // Verify hierarchy
        let root_children = engine.db.list_children(1).unwrap();
        assert_eq!(root_children.len(), 1);
        let docs = &root_children[0];
        assert_eq!(docs.name, "docs");

        let docs_children = engine.db.list_children(docs.inode).unwrap();
        assert_eq!(docs_children.len(), 2);

        let images = docs_children.iter().find(|e| e.name == "images").unwrap();
        let images_children = engine.db.list_children(images.inode).unwrap();
        assert_eq!(images_children.len(), 1);
        assert_eq!(images_children[0].name, "logo.png");
    }

    // 4. Second sync detects updates
    #[tokio::test]
    async fn second_sync_detects_updates() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir("", vec![make_remote("file.txt", false, "e1")]);

        let engine = test_engine(mock.clone());
        engine.full_sync().await.unwrap();

        // Change the etag on remote
        mock.add_dir("", vec![make_remote("file.txt", false, "e2")]);
        let report = engine.full_sync().await.unwrap();
        assert_eq!(report.updated, 1);
        assert_eq!(report.added, 0);

        // Verify updated etag in DB
        let children = engine.db.list_children(1).unwrap();
        assert_eq!(children[0].etag.as_deref(), Some("e2"));
    }

    // 5. Second sync detects deletions
    #[tokio::test]
    async fn second_sync_detects_deletions() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir(
            "",
            vec![
                make_remote("keep.txt", false, "e1"),
                make_remote("remove.txt", false, "e2"),
            ],
        );

        let engine = test_engine(mock.clone());
        engine.full_sync().await.unwrap();

        // Remove one file from remote
        mock.add_dir("", vec![make_remote("keep.txt", false, "e1")]);
        let report = engine.full_sync().await.unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(report.added, 0);
        assert_eq!(report.updated, 0);

        let children = engine.db.list_children(1).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].name, "keep.txt");
    }

    // 6. Backend error on subdirectory → other dirs still sync, error recorded
    #[tokio::test]
    async fn backend_error_continues_sync() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir(
            "",
            vec![
                make_remote("good", true, "e1"),
                make_remote("bad", true, "e2"),
            ],
        );
        mock.add_dir("good", vec![make_remote("ok.txt", false, "e3")]);
        mock.add_error_path("bad");

        let engine = test_engine(mock);
        let report = engine.full_sync().await.unwrap();

        // good dir and its file + bad dir = 3 added at root level
        assert_eq!(report.added, 3); // good, bad, ok.txt
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].path.contains("bad"));
    }

    // 7. SyncReport counts are accurate
    #[tokio::test]
    async fn sync_report_counts_accurate() {
        let mock = Arc::new(MockBackend::new());

        // First sync: add 3 entries
        mock.add_dir(
            "",
            vec![
                make_remote("a.txt", false, "e1"),
                make_remote("b.txt", false, "e2"),
                make_remote("c.txt", false, "e3"),
            ],
        );

        let engine = test_engine(mock.clone());
        let r1 = engine.full_sync().await.unwrap();
        assert_eq!(r1.added, 3);
        assert_eq!(r1.updated, 0);
        assert_eq!(r1.deleted, 0);
        assert!(r1.errors.is_empty());

        // Second sync: update 1, delete 1, add 1
        mock.add_dir(
            "",
            vec![
                make_remote("a.txt", false, "e1_v2"), // changed
                make_remote("b.txt", false, "e2"),    // unchanged
                // c.txt removed
                make_remote("d.txt", false, "e4"), // new
            ],
        );

        let r2 = engine.full_sync().await.unwrap();
        assert_eq!(r2.added, 1);
        assert_eq!(r2.updated, 1);
        assert_eq!(r2.deleted, 1);
        assert!(r2.errors.is_empty());
    }

    // 8. sync_dir with resolve_remote_path
    #[tokio::test]
    async fn sync_dir_resolves_path() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir("", vec![make_remote("docs", true, "e1")]);
        mock.add_dir("docs", vec![make_remote("file.txt", false, "e2")]);

        let engine = test_engine(mock);

        // First, full sync to populate the tree
        engine.full_sync().await.unwrap();

        let docs = engine.db.lookup(1, "docs").unwrap();

        // Now add a new file to remote docs
        engine.backend.dirs.lock().unwrap().insert(
            "docs".to_owned(),
            vec![
                make_remote("file.txt", false, "e2"),
                make_remote("new.txt", false, "e3"),
            ],
        );

        let report = engine.sync_dir(docs.inode).await.unwrap();
        assert_eq!(report.added, 1);

        let docs_children = engine.db.list_children(docs.inode).unwrap();
        assert_eq!(docs_children.len(), 2);
    }

    #[tokio::test]
    async fn always_local_auto_pins_matching_entries() {
        let mock = Arc::new(MockBackend::new());
        mock.add_dir(
            "",
            vec![
                make_remote("docs", true, "e1"),
                make_remote("photos", true, "e2"),
            ],
        );
        mock.add_dir("docs", vec![make_remote("readme.md", false, "e3")]);
        mock.add_dir("photos", vec![make_remote("cat.jpg", false, "e4")]);

        let db = Database::open_in_memory().expect("failed to open in-memory db");
        let engine = SyncEngine::new(db, mock, std::env::temp_dir(), vec!["docs".to_owned()]);

        engine.full_sync().await.unwrap();

        // docs dir and its contents should be pinned
        let root_children = engine.db.list_children(1).unwrap();
        let docs = root_children.iter().find(|e| e.name == "docs").unwrap();
        assert!(docs.is_pinned, "docs dir should be auto-pinned");

        let docs_children = engine.db.list_children(docs.inode).unwrap();
        let readme = docs_children
            .iter()
            .find(|e| e.name == "readme.md")
            .unwrap();
        assert!(readme.is_pinned, "docs/readme.md should be auto-pinned");

        // photos should NOT be pinned
        let photos = root_children.iter().find(|e| e.name == "photos").unwrap();
        assert!(!photos.is_pinned, "photos should not be pinned");
    }
}
