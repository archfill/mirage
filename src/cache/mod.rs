// LRU cache management.
//
// Manages locally cached file data with configurable capacity limits.
// Implements LRU eviction to stay within the configured cache size,
// while respecting pinned files that are excluded from eviction.

use std::path::PathBuf;
use std::sync::Mutex;

use bytes::Bytes;

use crate::db::Database;
use crate::db::models::SyncState;
use crate::error::Result;

/// Internal LRU tracking state, protected by Mutex.
struct LruState {
    /// LRU cache mapping inode -> cached file size in bytes
    cache: lru::LruCache<u64, u64>,
    /// Sum of all cached file sizes
    total_size: u64,
}

impl LruState {
    fn new() -> Self {
        Self {
            // Unbounded: eviction is managed by CacheManager based on total_size
            cache: lru::LruCache::unbounded(),
            total_size: 0,
        }
    }

    /// Insert a new entry or update an existing one (O(1)).
    fn insert(&mut self, inode: u64, size: u64) {
        if let Some(old_size) = self.cache.put(inode, size) {
            self.total_size = self.total_size - old_size + size;
        } else {
            self.total_size += size;
        }
    }

    /// Remove an entry from tracking (O(1)).
    fn remove(&mut self, inode: u64) {
        if let Some(size) = self.cache.pop(&inode) {
            self.total_size -= size;
        }
    }
}

pub struct CacheManager {
    cache_dir: PathBuf,
    cache_limit_bytes: u64,
    db: Database,
    inner: Mutex<LruState>,
}

impl CacheManager {
    /// Open the cache manager, rebuilding LRU state from existing files on disk.
    pub async fn open(cache_dir: PathBuf, cache_limit_bytes: u64, db: Database) -> Result<Self> {
        tokio::fs::create_dir_all(&cache_dir).await?;

        let mut entries: Vec<(u64, u64, std::time::SystemTime)> = Vec::new();

        let mut read_dir = tokio::fs::read_dir(&cache_dir).await?;
        while let Some(dir_entry) = read_dir.next_entry().await? {
            let file_name = dir_entry.file_name();
            let name_str = file_name.to_str().unwrap_or("");

            // Clean up orphaned write buffer temp files from previous crashes.
            if name_str.starts_with(".write_") {
                tracing::info!(file = name_str, "removing orphaned write buffer");
                let _ = tokio::fs::remove_file(dir_entry.path()).await;
                continue;
            }

            let Some(inode) = name_str.parse::<u64>().ok() else {
                continue;
            };
            let metadata = dir_entry.metadata().await?;
            let mtime = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            entries.push((inode, metadata.len(), mtime));
        }

        // Sort by mtime ascending: oldest (LRU) first
        entries.sort_by_key(|&(_, _, mtime)| mtime);

        let mut state = LruState::new();
        for (inode, size, _) in entries {
            state.cache.put(inode, size);
            state.total_size += size;
        }

        Ok(Self {
            cache_dir,
            cache_limit_bytes,
            db,
            inner: Mutex::new(state),
        })
    }

    /// Access the underlying database.
    pub fn db(&self) -> &Database {
        &self.db
    }

    /// Return the cache directory path.
    pub fn cache_dir(&self) -> &std::path::Path {
        &self.cache_dir
    }

    /// Return the on-disk path for a cached inode.
    fn file_path(&self, inode: u64) -> PathBuf {
        self.cache_dir.join(inode.to_string())
    }

    /// Get the path to a cached file, promoting it in the LRU.
    /// Returns `None` if the file is not cached.
    pub fn get(&self, inode: u64) -> Option<PathBuf> {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if state.cache.get(&inode).is_some() {
            Some(self.file_path(inode))
        } else {
            None
        }
    }

    /// Read the cached file contents, promoting it in the LRU.
    /// Returns `None` if the file is not cached.
    pub async fn read(&self, inode: u64) -> Result<Option<Bytes>> {
        let path = match self.get(inode) {
            Some(p) => p,
            None => return Ok(None),
        };
        let data = tokio::fs::read(&path).await?;
        Ok(Some(Bytes::from(data)))
    }

    /// Store data in the cache. Runs eviction if the cache exceeds its limit.
    /// Updates `db.set_cached(inode, true)`.
    pub async fn put(&self, inode: u64, data: &Bytes) -> Result<()> {
        let path = self.file_path(inode);
        tokio::fs::write(&path, data).await?;

        let size = data.len() as u64;
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.insert(inode, size);
        }

        self.db.set_cached(inode, true)?;

        if self.total_size() > self.cache_limit_bytes {
            self.evict().await?;
        }

        Ok(())
    }

    /// Remove a file from the cache.
    /// Updates `db.set_cached(inode, false)`.
    pub async fn remove(&self, inode: u64) -> Result<()> {
        let path = self.file_path(inode);
        if path.exists() {
            tokio::fs::remove_file(&path).await?;
        }
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            state.remove(inode);
        }
        self.db.set_cached(inode, false)?;
        Ok(())
    }

    /// Track a file that was written to the cache externally (e.g. by WriteBuffer::finalize).
    /// Updates the LRU state without writing data (the file is already on disk).
    pub fn track_external_put(&self, inode: u64, size: u64) {
        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.insert(inode, size);
    }

    /// Check whether a file is in the cache.
    pub fn contains(&self, inode: u64) -> bool {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.cache.contains(&inode)
    }

    /// Current total size of all cached files in bytes.
    pub fn total_size(&self) -> u64 {
        let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        state.total_size
    }

    /// Evict files in LRU order until total size is within the limit.
    /// Pinned files are skipped. Returns the number of files evicted.
    pub async fn evict(&self) -> Result<u64> {
        let mut evicted = 0u64;

        loop {
            // Collect candidates under lock
            let candidate = {
                let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                if state.total_size <= self.cache_limit_bytes {
                    break;
                }
                // Find the first non-pinned candidate from the LRU end
                let mut found = None;
                for (&inode, _) in state.cache.iter().rev() {
                    match self.db.get_by_inode(inode) {
                        Ok(entry)
                            if !entry.is_pinned && entry.sync_state != SyncState::PendingUpload =>
                        {
                            found = Some(inode);
                            break;
                        }
                        _ => continue,
                    }
                }
                found
            };

            match candidate {
                Some(inode) => {
                    let path = self.file_path(inode);
                    if path.exists() {
                        tokio::fs::remove_file(&path).await?;
                    }
                    {
                        let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
                        state.remove(inode);
                    }
                    self.db.set_cached(inode, false)?;
                    evicted += 1;
                }
                None => {
                    tracing::warn!("cache exceeds limit but all cached files are pinned");
                    break;
                }
            }
        }

        Ok(evicted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::{NewFileEntry, SyncState};

    fn sample_entry(parent: u64, name: &str) -> NewFileEntry {
        NewFileEntry {
            parent_inode: parent,
            name: name.to_owned(),
            is_dir: false,
            size: 0,
            permissions: 0o644,
            mtime: 1_700_000_000,
            etag: None,
            content_hash: None,
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::Synced,
        }
    }

    async fn setup_with_db(limit: u64, db: Database) -> (CacheManager, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().unwrap();
        let cm = CacheManager::open(tmp.path().to_path_buf(), limit, db)
            .await
            .unwrap();
        (cm, tmp)
    }

    #[tokio::test]
    async fn put_and_get() {
        let db = Database::open_in_memory().unwrap();
        let inode = db.insert(&sample_entry(1, "file.txt")).unwrap();
        let (cm, _tmp) = setup_with_db(1024, db).await;

        let data = Bytes::from(vec![1u8; 100]);
        cm.put(inode, &data).await.unwrap();

        assert!(cm.contains(inode));
        assert_eq!(cm.total_size(), 100);

        let path = cm.get(inode).unwrap();
        assert!(path.exists());

        // Verify DB is_cached flag
        let entry = cm.db.get_by_inode(inode).unwrap();
        assert!(entry.is_cached);
    }

    #[tokio::test]
    async fn read_returns_bytes() {
        let db = Database::open_in_memory().unwrap();
        let inode = db.insert(&sample_entry(1, "data.bin")).unwrap();
        let (cm, _tmp) = setup_with_db(1024, db).await;

        // Not cached yet
        assert!(cm.read(inode).await.unwrap().is_none());

        let data = Bytes::from(b"hello world".to_vec());
        cm.put(inode, &data).await.unwrap();

        let result = cm.read(inode).await.unwrap().unwrap();
        assert_eq!(result, data);
    }

    #[tokio::test]
    async fn remove_deletes_file() {
        let db = Database::open_in_memory().unwrap();
        let inode = db.insert(&sample_entry(1, "rm.txt")).unwrap();
        let (cm, _tmp) = setup_with_db(1024, db).await;

        let data = Bytes::from(vec![0u8; 50]);
        cm.put(inode, &data).await.unwrap();
        cm.remove(inode).await.unwrap();

        assert!(!cm.contains(inode));
        assert_eq!(cm.total_size(), 0);
        assert!(!cm.file_path(inode).exists());

        let entry = cm.db.get_by_inode(inode).unwrap();
        assert!(!entry.is_cached);
    }

    #[tokio::test]
    async fn eviction_lru_order() {
        let db = Database::open_in_memory().unwrap();
        let i1 = db.insert(&sample_entry(1, "a.txt")).unwrap();
        let i2 = db.insert(&sample_entry(1, "b.txt")).unwrap();
        let i3 = db.insert(&sample_entry(1, "c.txt")).unwrap();
        let (cm, _tmp) = setup_with_db(100, db).await;

        // Put 3 files of 50 bytes each (total 150 > limit 100)
        cm.put(i1, &Bytes::from(vec![1u8; 50])).await.unwrap();
        cm.put(i2, &Bytes::from(vec![2u8; 50])).await.unwrap();
        cm.put(i3, &Bytes::from(vec![3u8; 50])).await.unwrap();

        // i1 (oldest) should have been evicted
        assert!(!cm.contains(i1));
        assert!(cm.contains(i2));
        assert!(cm.contains(i3));
        assert_eq!(cm.total_size(), 100);
    }

    #[tokio::test]
    async fn eviction_skips_pinned() {
        let db = Database::open_in_memory().unwrap();
        let i1 = db.insert(&sample_entry(1, "pinned.txt")).unwrap();
        let i2 = db.insert(&sample_entry(1, "normal.txt")).unwrap();
        let i3 = db.insert(&sample_entry(1, "new.txt")).unwrap();
        db.set_pinned(i1, true).unwrap();
        let (cm, _tmp) = setup_with_db(100, db).await;

        cm.put(i1, &Bytes::from(vec![1u8; 50])).await.unwrap();
        cm.put(i2, &Bytes::from(vec![2u8; 50])).await.unwrap();
        cm.put(i3, &Bytes::from(vec![3u8; 50])).await.unwrap();

        // i1 is pinned so i2 (next LRU) should be evicted instead
        assert!(cm.contains(i1));
        assert!(!cm.contains(i2));
        assert!(cm.contains(i3));
    }

    #[tokio::test]
    async fn get_promotes_to_mru() {
        let db = Database::open_in_memory().unwrap();
        let i1 = db.insert(&sample_entry(1, "a.txt")).unwrap();
        let i2 = db.insert(&sample_entry(1, "b.txt")).unwrap();
        let i3 = db.insert(&sample_entry(1, "c.txt")).unwrap();
        let (cm, _tmp) = setup_with_db(100, db).await;

        cm.put(i1, &Bytes::from(vec![1u8; 50])).await.unwrap();
        cm.put(i2, &Bytes::from(vec![2u8; 50])).await.unwrap();

        // Access i1 to promote it to MRU
        cm.get(i1);

        // Now put i3 which triggers eviction — i2 should be evicted (it's now LRU)
        cm.put(i3, &Bytes::from(vec![3u8; 50])).await.unwrap();

        assert!(cm.contains(i1));
        assert!(!cm.contains(i2));
        assert!(cm.contains(i3));
    }

    #[tokio::test]
    async fn open_rebuilds_from_disk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cache_dir = tmp.path().to_path_buf();

        // Pre-populate cache files on disk
        tokio::fs::write(cache_dir.join("10"), vec![0u8; 30])
            .await
            .unwrap();
        tokio::fs::write(cache_dir.join("20"), vec![0u8; 70])
            .await
            .unwrap();

        let db = Database::open_in_memory().unwrap();
        let cm = CacheManager::open(cache_dir, 1024, db).await.unwrap();

        assert!(cm.contains(10));
        assert!(cm.contains(20));
        assert_eq!(cm.total_size(), 100);
    }

    #[tokio::test]
    async fn put_overwrites_existing() {
        let db = Database::open_in_memory().unwrap();
        let inode = db.insert(&sample_entry(1, "file.txt")).unwrap();
        let (cm, _tmp) = setup_with_db(1024, db).await;

        cm.put(inode, &Bytes::from(vec![0u8; 100])).await.unwrap();
        assert_eq!(cm.total_size(), 100);

        // Overwrite with smaller data
        cm.put(inode, &Bytes::from(vec![0u8; 40])).await.unwrap();
        assert_eq!(cm.total_size(), 40);

        let content = cm.read(inode).await.unwrap().unwrap();
        assert_eq!(content.len(), 40);
    }

    #[tokio::test]
    async fn evict_all_pinned_stops() {
        let db = Database::open_in_memory().unwrap();
        let i1 = db.insert(&sample_entry(1, "a.txt")).unwrap();
        let i2 = db.insert(&sample_entry(1, "b.txt")).unwrap();
        db.set_pinned(i1, true).unwrap();
        db.set_pinned(i2, true).unwrap();
        // Use a limit smaller than what we'll put in
        let (cm, _tmp) = setup_with_db(50, db).await;

        cm.put(i1, &Bytes::from(vec![0u8; 40])).await.unwrap();
        cm.put(i2, &Bytes::from(vec![0u8; 40])).await.unwrap();

        // Both pinned — evict should return 0
        let evicted = cm.evict().await.unwrap();
        assert_eq!(evicted, 0);
        assert!(cm.contains(i1));
        assert!(cm.contains(i2));
    }
}
