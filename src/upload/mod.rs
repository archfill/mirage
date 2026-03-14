// Upload worker for background file synchronization.
//
// Processes upload, delete, move, and create-dir operations on a dedicated
// thread. Communicates with the FUSE layer via a synchronous channel, and
// drives async backend calls through a supplied Tokio runtime handle.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;

use crate::backend::Backend;
use crate::db::Database;
use crate::db::models::SyncState;
use crate::error::Result;

/// Messages sent to the `UploadWorker`.
pub enum UploadMessage {
    /// Upload the cached file for this inode to the remote backend.
    Upload(u64),
    /// Delete the remote entry at the given path.
    Delete(String),
    /// Move/rename a remote entry.
    Move { from: String, to: String },
    /// Create a remote directory.
    CreateDir(String),
    /// Shut down the worker loop.
    Shutdown,
}

/// Background worker that processes upload operations.
///
/// Runs on a dedicated OS thread. Async backend calls are driven via the
/// `rt` handle so the worker does not block the Tokio thread pool.
pub struct UploadWorker<B: Backend> {
    pub db: Database,
    pub backend: Arc<B>,
    pub cache_dir: PathBuf,
    pub rx: std::sync::mpsc::Receiver<UploadMessage>,
    pub rt: tokio::runtime::Handle,
    pub retry_base_secs: u64,
    pub retry_max_secs: u64,
    /// Track retry state per inode: (attempt_count, next_retry_time)
    retry_delays: std::collections::HashMap<u64, (u32, std::time::Instant)>,
    network_state: Arc<std::sync::atomic::AtomicU8>,
}

impl<B: Backend + 'static> UploadWorker<B> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Database,
        backend: Arc<B>,
        cache_dir: PathBuf,
        rx: std::sync::mpsc::Receiver<UploadMessage>,
        rt: tokio::runtime::Handle,
        retry_base_secs: u64,
        retry_max_secs: u64,
        network_state: Arc<std::sync::atomic::AtomicU8>,
    ) -> Self {
        Self {
            db,
            backend,
            cache_dir,
            rx,
            rt,
            retry_base_secs,
            retry_max_secs,
            retry_delays: std::collections::HashMap::new(),
            network_state,
        }
    }

    /// Run the worker loop until a `Shutdown` message is received.
    ///
    /// On each iteration the worker either processes an incoming message or,
    /// after a 30-second timeout, retries any entries still marked
    /// `PendingUpload` in the database.
    pub fn run(mut self) {
        loop {
            match self.rx.recv_timeout(Duration::from_secs(30)) {
                Ok(UploadMessage::Upload(inode)) => {
                    if let Err(e) = self.handle_upload(inode) {
                        tracing::error!(inode, error = %e, "upload failed");
                    }
                }
                Ok(UploadMessage::Delete(path)) => {
                    let result = self.rt.block_on(self.backend.delete(&path));
                    if let Err(e) = result {
                        tracing::error!(path, error = %e, "delete failed");
                    }
                }
                Ok(UploadMessage::Move { from, to }) => {
                    let result = self.rt.block_on(self.backend.move_entry(&from, &to));
                    if let Err(e) = result {
                        tracing::error!(from, to, error = %e, "move failed");
                    }
                }
                Ok(UploadMessage::CreateDir(path)) => {
                    let result = self.rt.block_on(self.backend.create_dir(&path));
                    if let Err(e) = result {
                        tracing::error!(path, error = %e, "create_dir failed");
                    }
                }
                Ok(UploadMessage::Shutdown) => {
                    tracing::info!("upload worker shutting down, draining remaining messages");
                    // Drain any remaining messages in the channel.
                    while let Ok(msg) = self.rx.try_recv() {
                        match msg {
                            UploadMessage::Upload(inode) => {
                                if let Err(e) = self.handle_upload(inode) {
                                    tracing::error!(inode, error = %e, "shutdown drain: upload failed");
                                }
                            }
                            UploadMessage::Delete(path) => {
                                let result = self.rt.block_on(self.backend.delete(&path));
                                if let Err(e) = result {
                                    tracing::error!(path, error = %e, "shutdown drain: delete failed");
                                }
                            }
                            UploadMessage::Move { from, to } => {
                                let result = self.rt.block_on(self.backend.move_entry(&from, &to));
                                if let Err(e) = result {
                                    tracing::error!(from, to, error = %e, "shutdown drain: move failed");
                                }
                            }
                            UploadMessage::CreateDir(path) => {
                                let result = self.rt.block_on(self.backend.create_dir(&path));
                                if let Err(e) = result {
                                    tracing::error!(path, error = %e, "shutdown drain: create_dir failed");
                                }
                            }
                            UploadMessage::Shutdown => {}
                        }
                    }
                    // Final retry of any pending uploads still in the database.
                    self.retry_pending();
                    tracing::info!("upload worker shutdown complete");
                    break;
                }

                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    self.retry_pending();
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    tracing::warn!("upload worker channel disconnected, exiting");
                    break;
                }
            }
        }
    }

    /// Retry all entries currently marked `PendingUpload`, applying exponential backoff.
    fn retry_pending(&mut self) {
        // Skip retry when offline
        if self
            .network_state
            .load(std::sync::atomic::Ordering::Relaxed)
            != 0
        {
            return;
        }
        let entries = match self.db.get_by_sync_state(SyncState::PendingUpload) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "failed to query pending uploads");
                return;
            }
        };
        let now = std::time::Instant::now();
        for entry in entries {
            // Check if backoff period has elapsed.
            if let Some((_, next_retry)) = self.retry_delays.get(&entry.inode)
                && now < *next_retry
            {
                continue;
            }
            match self.handle_upload(entry.inode) {
                Ok(()) => {
                    self.retry_delays.remove(&entry.inode);
                }
                Err(e) => {
                    if !e.is_transient() {
                        tracing::error!(inode = entry.inode, error = %e, "permanent upload error, will not retry");
                        self.retry_delays.remove(&entry.inode);
                        continue;
                    }
                    let prev_count = self.retry_delays.get(&entry.inode).map_or(0, |(c, _)| *c);
                    let new_count = prev_count + 1;
                    let delay_secs = std::cmp::min(
                        self.retry_base_secs * 2u64.saturating_pow(new_count),
                        self.retry_max_secs,
                    );
                    let next = now + std::time::Duration::from_secs(delay_secs);
                    self.retry_delays.insert(entry.inode, (new_count, next));
                    tracing::warn!(
                        inode = entry.inode,
                        attempt = new_count,
                        next_retry_secs = delay_secs,
                        error = %e,
                        "transient upload error, backing off"
                    );
                }
            }
        }
    }

    /// Upload the locally cached file for `inode` to the remote backend,
    /// then update the database with the returned metadata.
    fn handle_upload(&self, inode: u64) -> Result<()> {
        let entry = self.db.get_by_inode(inode)?;
        let remote_path = self.build_remote_path(inode)?;

        if entry.is_dir {
            self.rt.block_on(self.backend.create_dir(&remote_path))?;
            self.db.update_sync_state(inode, SyncState::Synced)?;
            tracing::info!(inode, path = remote_path, "directory sync completed");
            return Ok(());
        }

        let cache_file = self.cache_dir.join(inode.to_string());
        let data = std::fs::read(&cache_file).map_err(crate::error::Error::Io)?;
        let bytes = Bytes::from(data);

        let remote_entry = match self.rt.block_on(self.backend.upload(&remote_path, bytes)) {
            Ok(entry) => entry,
            Err(e) => {
                if e.is_transient() {
                    self.network_state
                        .store(1, std::sync::atomic::Ordering::Relaxed);
                }
                return Err(e);
            }
        };

        let updated = crate::db::models::NewFileEntry {
            parent_inode: entry.parent_inode,
            name: entry.name.clone(),
            is_dir: entry.is_dir,
            size: entry.size,
            permissions: entry.permissions,
            mtime: entry.mtime,
            etag: remote_entry.etag.clone(),
            content_hash: remote_entry.content_hash.clone(),
            is_pinned: entry.is_pinned,
            is_cached: entry.is_cached,
            sync_state: SyncState::Synced,
        };
        self.db.update_metadata(inode, &updated)?;

        self.network_state
            .store(0, std::sync::atomic::Ordering::Relaxed);
        tracing::info!(inode, path = remote_path, "upload completed");
        Ok(())
    }

    /// Build the remote path for an inode by walking the parent chain in the
    /// database up to the root (inode 1).
    pub fn build_remote_path(&self, inode: u64) -> Result<String> {
        let mut components: Vec<String> = Vec::new();
        let mut current = inode;

        loop {
            let entry = self.db.get_by_inode(current)?;
            // Root inode points to itself as parent and has an empty name.
            if entry.inode == 1 {
                break;
            }
            if !entry.name.is_empty() {
                components.push(entry.name.clone());
            }
            if entry.parent_inode == entry.inode {
                // Safety stop: detected self-referential root.
                break;
            }
            current = entry.parent_inode;
        }

        components.reverse();
        Ok(components.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU8;

    use bytes::Bytes;

    use super::UploadWorker;
    use crate::backend::{Backend, RemoteEntry};
    use crate::db::Database;
    use crate::db::models::{NewFileEntry, SyncState};
    use crate::error::Result;

    // ── MockBackend ───────────────────────────────────────────────────────────

    struct MockBackend {
        upload_calls: std::sync::Mutex<Vec<(String, Bytes)>>,
        create_dir_calls: std::sync::Mutex<Vec<String>>,
    }

    impl MockBackend {
        fn new() -> Self {
            Self {
                upload_calls: std::sync::Mutex::new(Vec::new()),
                create_dir_calls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Backend for MockBackend {
        fn list_dir(
            &self,
            _remote_path: &str,
        ) -> impl std::future::Future<Output = Result<Vec<RemoteEntry>>> + Send {
            std::future::ready(Err(crate::error::Error::Config("not implemented".into())))
        }

        fn get_metadata(
            &self,
            _remote_path: &str,
        ) -> impl std::future::Future<Output = Result<RemoteEntry>> + Send {
            std::future::ready(Err(crate::error::Error::Config("not implemented".into())))
        }

        fn download(
            &self,
            _remote_path: &str,
        ) -> impl std::future::Future<Output = Result<Bytes>> + Send {
            std::future::ready(Err(crate::error::Error::Config("not implemented".into())))
        }

        fn upload(
            &self,
            remote_path: &str,
            data: Bytes,
        ) -> impl std::future::Future<Output = Result<RemoteEntry>> + Send {
            self.upload_calls
                .lock()
                .unwrap()
                .push((remote_path.to_owned(), data));

            let entry = RemoteEntry {
                path: remote_path.to_owned(),
                is_dir: false,
                size: 0,
                mtime: 0,
                etag: Some("new-etag".to_owned()),
                content_hash: Some("new-hash".to_owned()),
                content_type: None,
            };
            std::future::ready(Ok(entry))
        }

        fn delete(
            &self,
            _remote_path: &str,
        ) -> impl std::future::Future<Output = Result<()>> + Send {
            std::future::ready(Err(crate::error::Error::Config("not implemented".into())))
        }

        fn move_entry(
            &self,
            _from: &str,
            _to: &str,
        ) -> impl std::future::Future<Output = Result<()>> + Send {
            std::future::ready(Err(crate::error::Error::Config("not implemented".into())))
        }

        fn create_dir(
            &self,
            remote_path: &str,
        ) -> impl std::future::Future<Output = Result<()>> + Send {
            self.create_dir_calls
                .lock()
                .unwrap()
                .push(remote_path.to_owned());
            std::future::ready(Ok(()))
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn sample_entry(parent: u64, name: &str, is_dir: bool) -> NewFileEntry {
        NewFileEntry {
            parent_inode: parent,
            name: name.to_owned(),
            is_dir,
            size: if is_dir { 0 } else { 1024 },
            permissions: if is_dir { 0o755 } else { 0o644 },
            mtime: 1_700_000_000,
            etag: None,
            content_hash: None,
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::PendingUpload,
        }
    }

    fn test_worker(
        db: Database,
        cache_dir: PathBuf,
        backend: Arc<MockBackend>,
    ) -> UploadWorker<MockBackend> {
        let (_tx, rx) = std::sync::mpsc::channel();
        // Leak the runtime so its handle remains valid for the duration of the test.
        let rt = Box::leak(Box::new(tokio::runtime::Runtime::new().unwrap()));
        let network_state = Arc::new(AtomicU8::new(0));
        UploadWorker::new(
            db,
            backend,
            cache_dir,
            rx,
            rt.handle().clone(),
            30,
            600,
            network_state,
        )
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_build_remote_path_root_child() {
        let db = Database::open_in_memory().unwrap();
        let backend = Arc::new(MockBackend::new());
        let cache_dir = std::env::temp_dir();
        let inode = db.insert(&sample_entry(1, "file.txt", false)).unwrap();
        let worker = test_worker(db, cache_dir, backend);

        let path = worker.build_remote_path(inode).unwrap();
        assert_eq!(path, "file.txt");
    }

    #[test]
    fn test_build_remote_path_nested() {
        let db = Database::open_in_memory().unwrap();
        let backend = Arc::new(MockBackend::new());
        let cache_dir = std::env::temp_dir();

        let dir_inode = db.insert(&sample_entry(1, "docs", true)).unwrap();
        let file_inode = db
            .insert(&sample_entry(dir_inode, "readme.md", false))
            .unwrap();

        let worker = test_worker(db, cache_dir, backend);
        let path = worker.build_remote_path(file_inode).unwrap();
        assert_eq!(path, "docs/readme.md");
    }

    #[test]
    fn test_build_remote_path_root() {
        let db = Database::open_in_memory().unwrap();
        let backend = Arc::new(MockBackend::new());
        let cache_dir = std::env::temp_dir();
        let worker = test_worker(db, cache_dir, backend);

        let path = worker.build_remote_path(1).unwrap();
        assert_eq!(path, "");
    }

    #[test]
    fn test_handle_upload_file() {
        let db = Database::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let backend = Arc::new(MockBackend::new());

        let inode = db.insert(&sample_entry(1, "hello.txt", false)).unwrap();
        std::fs::write(cache_dir.join(inode.to_string()), b"hello").unwrap();

        let worker = test_worker(db, cache_dir, Arc::clone(&backend));
        worker.handle_upload(inode).unwrap();

        let calls = backend.upload_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "hello.txt");
        assert_eq!(calls[0].1, Bytes::from_static(b"hello"));
        drop(calls);

        let entry = worker.db.get_by_inode(inode).unwrap();
        assert_eq!(entry.sync_state, SyncState::Synced);
        assert_eq!(entry.etag.as_deref(), Some("new-etag"));
    }

    #[test]
    fn test_handle_upload_directory() {
        let db = Database::open_in_memory().unwrap();
        let cache_dir = std::env::temp_dir();
        let backend = Arc::new(MockBackend::new());

        let inode = db.insert(&sample_entry(1, "myfolder", true)).unwrap();

        let worker = test_worker(db, cache_dir, Arc::clone(&backend));
        worker.handle_upload(inode).unwrap();

        let calls = backend.create_dir_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "myfolder");
        drop(calls);

        let entry = worker.db.get_by_inode(inode).unwrap();
        assert_eq!(entry.sync_state, SyncState::Synced);
    }

    #[test]
    fn test_retry_pending_mixed() {
        let db = Database::open_in_memory().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let backend = Arc::new(MockBackend::new());

        let file_inode = db.insert(&sample_entry(1, "data.bin", false)).unwrap();
        std::fs::write(cache_dir.join(file_inode.to_string()), b"hello").unwrap();

        let dir_inode = db.insert(&sample_entry(1, "newdir", true)).unwrap();

        let mut worker = test_worker(db, cache_dir, Arc::clone(&backend));
        worker.retry_pending();

        let file_entry = worker.db.get_by_inode(file_inode).unwrap();
        assert_eq!(file_entry.sync_state, SyncState::Synced);

        let dir_entry = worker.db.get_by_inode(dir_inode).unwrap();
        assert_eq!(dir_entry.sync_state, SyncState::Synced);

        let upload_calls = backend.upload_calls.lock().unwrap();
        assert_eq!(upload_calls.len(), 1);
        drop(upload_calls);

        let create_dir_calls = backend.create_dir_calls.lock().unwrap();
        assert_eq!(create_dir_calls.len(), 1);
    }
}
