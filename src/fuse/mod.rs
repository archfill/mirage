// FUSE filesystem implementation (read-write, Phase 2).
//
// Handles FUSE callbacks (getattr, readdir, open, read, write, create,
// release, setattr, mkdir, unlink, rmdir, rename, flush, fsync)
// by delegating to the local DB for metadata and to the backend
// for file content downloads/uploads.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fuser::{
    BsdFileFlags, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, LockOwner, MountOption, OpenAccMode, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request,
    WriteFlags,
};

use crate::backend::Backend;
use crate::cache::CacheManager;
use crate::db::models::{FileEntry, NewFileEntry, SyncState};
use crate::error::Error;
use crate::upload::UploadMessage;
use libc;

mod write_buffer;
use write_buffer::WriteBuffer;

const TTL: Duration = Duration::from_secs(1);

pub struct MirageFs<B: Backend> {
    cache: Mutex<CacheManager>,
    backend: Arc<B>,
    rt: tokio::runtime::Handle,
    uid: u32,
    gid: u32,
    write_bufs: Mutex<HashMap<(u64, u64), WriteBuffer>>,
    next_fh: AtomicU64,
    upload_tx: std::sync::mpsc::Sender<UploadMessage>,
    network_state: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

impl<B: Backend + 'static> MirageFs<B> {
    pub fn new(
        cache: CacheManager,
        backend: Arc<B>,
        rt: tokio::runtime::Handle,
        uid: u32,
        gid: u32,
        upload_tx: std::sync::mpsc::Sender<UploadMessage>,
        network_state: std::sync::Arc<std::sync::atomic::AtomicU8>,
    ) -> Self {
        Self {
            cache: Mutex::new(cache),
            backend,
            rt,
            uid,
            gid,
            write_bufs: Mutex::new(HashMap::new()),
            next_fh: AtomicU64::new(1),
            upload_tx,
            network_state,
        }
    }

    /// Mount the filesystem at the given path (blocking).
    pub fn mount(self, mountpoint: &std::path::Path) -> crate::error::Result<()> {
        let mut config = Config::default();
        config.mount_options = vec![MountOption::FSName("mirage".into())];
        fuser::mount2(self, mountpoint, &config)?;
        Ok(())
    }

    fn to_file_attr(&self, entry: &FileEntry) -> FileAttr {
        let kind = if entry.is_dir {
            FileType::Directory
        } else {
            FileType::RegularFile
        };
        let mtime = UNIX_EPOCH + Duration::from_secs(entry.mtime.max(0) as u64);
        let nlink = if entry.is_dir { 2 } else { 1 };
        let perm = entry.permissions as u16;
        FileAttr {
            ino: INodeNo(entry.inode),
            size: entry.size,
            blocks: entry.size.div_ceil(512),
            atime: mtime,
            mtime,
            ctime: mtime,
            crtime: mtime,
            kind,
            perm,
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }

    /// Build the remote path for an inode by walking up the parent chain.
    fn build_remote_path(&self, inode: u64, cache: &CacheManager) -> crate::error::Result<String> {
        let mut parts = Vec::new();
        let mut current = inode;
        loop {
            let entry = cache.db().get_by_inode(current)?;
            if entry.inode == 1 {
                break;
            }
            parts.push(entry.name);
            current = entry.parent_inode;
        }
        parts.reverse();
        Ok(parts.join("/"))
    }

    /// Get file attributes for an inode (internal, testable).
    #[cfg(test)]
    fn do_getattr(&self, ino: u64) -> std::result::Result<FileAttr, Errno> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.db().get_by_inode(ino) {
            Ok(entry) => Ok(self.to_file_attr(&entry)),
            Err(Error::InodeNotFound(_)) => Err(Errno::ENOENT),
            Err(e) => {
                tracing::error!(inode = ino, error = %e, "do_getattr failed");
                Err(Errno::EIO)
            }
        }
    }

    /// Lookup a child entry by name (internal, testable).
    #[cfg(test)]
    fn do_lookup(&self, parent: u64, name: &str) -> std::result::Result<FileAttr, Errno> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.db().lookup(parent, name) {
            Ok(entry) => Ok(self.to_file_attr(&entry)),
            Err(Error::EntryNotFound(_, _)) => Err(Errno::ENOENT),
            Err(e) => {
                tracing::error!(parent, name, error = %e, "do_lookup failed");
                Err(Errno::EIO)
            }
        }
    }

    /// List directory entries (internal, testable).
    #[cfg(test)]
    fn do_readdir(&self, ino: u64) -> std::result::Result<Vec<(u64, bool, String)>, Errno> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let parent_inode = match cache.db().get_by_inode(ino) {
            Ok(entry) => entry.parent_inode,
            Err(Error::InodeNotFound(_)) => return Err(Errno::ENOENT),
            Err(e) => {
                tracing::error!(inode = ino, error = %e, "do_readdir: getattr failed");
                return Err(Errno::EIO);
            }
        };

        let children = match cache.db().list_children(ino) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(inode = ino, error = %e, "do_readdir: list_children failed");
                return Err(Errno::EIO);
            }
        };

        let mut entries = vec![
            (ino, true, ".".to_string()),
            (parent_inode, true, "..".to_string()),
        ];
        for child in children {
            entries.push((child.inode, child.is_dir, child.name));
        }
        Ok(entries)
    }

    /// Flush write buffer for (inode, fh) to cache and update DB.
    fn flush_buffer(&self, ino: u64, fh: u64) -> std::result::Result<(), Errno> {
        let mut bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(buf) = bufs.remove(&(ino, fh)) {
            let buf_len = buf.len();
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            let target = cache.cache_dir().join(ino.to_string());
            buf.finalize(&target).map_err(|e| {
                tracing::error!(inode = ino, error = %e, "flush: finalize failed");
                Errno::EIO
            })?;

            // Update LRU state and DB
            cache.track_external_put(ino, buf_len);
            cache.db().set_cached(ino, true).map_err(|e| {
                tracing::error!(inode = ino, error = %e, "flush: set_cached failed");
                Errno::EIO
            })?;

            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            cache
                .db()
                .update_file_after_write(ino, buf_len, now)
                .map_err(|e| {
                    tracing::error!(inode = ino, error = %e, "flush: db update failed");
                    Errno::EIO
                })?;

            // Re-create WriteBuffer from the flushed file (no memory copy).
            let cache_dir = cache.cache_dir().to_path_buf();
            let new_buf =
                WriteBuffer::from_existing(&cache_dir, ino, fh, &target).map_err(|e| {
                    tracing::error!(inode = ino, error = %e, "flush: re-create WriteBuffer failed");
                    Errno::EIO
                })?;
            bufs.insert((ino, fh), new_buf);
        }

        Ok(())
    }
}

impl<B: Backend + 'static> Filesystem for MirageFs<B> {
    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let ino_u64: u64 = ino.into();
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.db().get_by_inode(ino_u64) {
            Ok(entry) => reply.attr(&TTL, &self.to_file_attr(&entry)),
            Err(Error::InodeNotFound(_)) => reply.error(Errno::ENOENT),
            Err(e) => {
                tracing::error!(inode = ino_u64, error = %e, "getattr failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let parent_u64: u64 = parent.into();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match cache.db().lookup(parent_u64, name_str) {
            Ok(entry) => {
                let attr = self.to_file_attr(&entry);
                reply.entry(&TTL, &attr, Generation(0));
            }
            Err(Error::EntryNotFound(_, _)) => reply.error(Errno::ENOENT),
            Err(e) => {
                tracing::error!(parent = parent_u64, name = name_str, error = %e, "lookup failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let ino_u64: u64 = ino.into();
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());

        let parent_inode = match cache.db().get_by_inode(ino_u64) {
            Ok(entry) => entry.parent_inode,
            Err(Error::InodeNotFound(_)) => {
                reply.error(Errno::ENOENT);
                return;
            }
            Err(e) => {
                tracing::error!(inode = ino_u64, error = %e, "readdir: getattr failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        let children = match cache.db().list_children(ino_u64) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(inode = ino_u64, error = %e, "readdir: list_children failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        let dot_entries: Vec<(u64, FileType, String)> = vec![
            (ino_u64, FileType::Directory, ".".into()),
            (parent_inode, FileType::Directory, "..".into()),
        ];

        let all_entries = dot_entries.into_iter().chain(children.iter().map(|e| {
            let kind = if e.is_dir {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            (e.inode, kind, e.name.clone())
        }));

        for (i, (inode, kind, name)) in all_entries.enumerate().skip(offset as usize) {
            let next_offset = (i + 1) as u64;
            if reply.add(INodeNo(inode), next_offset, kind, &name) {
                break;
            }
        }
        reply.ok();
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let ino_u64: u64 = ino.into();

        // Phase 1: Lock → check cache + build remote path → Unlock
        let (needs_download, remote_path) = {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());

            let entry = match cache.db().get_by_inode(ino_u64) {
                Ok(e) => e,
                Err(Error::InodeNotFound(_)) => {
                    reply.error(Errno::ENOENT);
                    return;
                }
                Err(e) => {
                    tracing::error!(inode = ino_u64, error = %e, "open: getattr failed");
                    reply.error(Errno::EIO);
                    return;
                }
            };

            if entry.is_dir {
                reply.error(Errno::EISDIR);
                return;
            }

            if cache.contains(ino_u64) {
                (false, String::new())
            } else {
                match self.build_remote_path(ino_u64, &cache) {
                    Ok(p) => (true, p),
                    Err(e) => {
                        tracing::error!(inode = ino_u64, error = %e, "open: build_remote_path failed");
                        reply.error(Errno::EIO);
                        return;
                    }
                }
            }
        };

        // Phase 2: Download without holding Mutex (other FUSE ops can proceed)
        if needs_download {
            // If offline, fail fast instead of blocking on network timeout
            if self
                .network_state
                .load(std::sync::atomic::Ordering::Relaxed)
                != 0
            {
                reply.error(Errno::from_i32(libc::EHOSTUNREACH));
                return;
            }
            let backend = Arc::clone(&self.backend);
            let data = match self.rt.block_on(backend.download(&remote_path)) {
                Ok(d) => d,
                Err(e) => {
                    tracing::error!(inode = ino_u64, error = %e, "open: download failed");
                    reply.error(Errno::EIO);
                    return;
                }
            };

            // Phase 3: Re-lock → cache.put
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = self.rt.block_on(cache.put(ino_u64, &data)) {
                tracing::error!(inode = ino_u64, error = %e, "open: cache put failed");
                reply.error(Errno::EIO);
                return;
            }
        }

        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);

        // Initialize write buffer if opened for writing
        let acc_mode = flags.acc_mode();
        if acc_mode == OpenAccMode::O_WRONLY || acc_mode == OpenAccMode::O_RDWR {
            let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            let existing_data = match self.rt.block_on(cache.read(ino_u64)) {
                Ok(Some(data)) => Some(data),
                _ => None,
            };
            let cache_dir = cache.cache_dir().to_path_buf();
            let wb = match crate::fuse::write_buffer::WriteBuffer::new(
                &cache_dir,
                ino_u64,
                fh,
                existing_data.as_deref(),
            ) {
                Ok(wb) => wb,
                Err(e) => {
                    tracing::error!(inode = ino_u64, error = %e, "open: WriteBuffer creation failed");
                    reply.error(Errno::EIO);
                    return;
                }
            };
            let mut bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
            bufs.insert((ino_u64, fh), wb);
        }

        reply.opened(FileHandle(fh), FopenFlags::empty());
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let ino_u64: u64 = ino.into();
        let fh_u64: u64 = fh.into();

        // Check write buffer first for read-after-write consistency
        {
            let mut bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(buf) = bufs.get_mut(&(ino_u64, fh_u64)) {
                match buf.read_at(offset, size) {
                    Ok(data) => reply.data(&data),
                    Err(e) => {
                        tracing::error!(inode = ino_u64, error = %e, "read: write buffer read failed");
                        reply.error(Errno::EIO);
                    }
                }
                return;
            }
        }

        // No write buffer — read from cache
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        match self.rt.block_on(cache.read(ino_u64)) {
            Ok(Some(data)) => {
                let start = offset as usize;
                if start >= data.len() {
                    reply.data(&[]);
                } else {
                    let end = (start + size as usize).min(data.len());
                    reply.data(&data[start..end]);
                }
            }
            Ok(None) => {
                reply.error(Errno::ENOENT);
            }
            Err(e) => {
                tracing::error!(inode = ino_u64, error = %e, "read failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let ino_u64: u64 = ino.into();
        let fh_u64: u64 = fh.into();

        let mut bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
        let buf = match bufs.get_mut(&(ino_u64, fh_u64)) {
            Some(b) => b,
            None => {
                reply.error(Errno::EBADF);
                return;
            }
        };

        match buf.write_at(offset, data) {
            Ok(written) => reply.written(written),
            Err(e) => {
                tracing::error!(inode = ino_u64, error = %e, "write failed");
                reply.error(Errno::EIO);
            }
        }
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let ino_u64: u64 = ino.into();
        let fh_u64: u64 = fh.into();

        let had_buffer = {
            let bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
            bufs.contains_key(&(ino_u64, fh_u64))
        };

        if had_buffer {
            if let Err(errno) = self.flush_buffer(ino_u64, fh_u64) {
                reply.error(errno);
                return;
            }
            // flush_buffer re-inserts the buffer for continued writes; on release, remove it.
            let mut bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
            bufs.remove(&(ino_u64, fh_u64));
            if let Err(e) = self.upload_tx.send(UploadMessage::Upload(ino_u64)) {
                tracing::error!(inode = ino_u64, error = %e, "upload channel disconnected");
            }
        }

        reply.ok();
    }

    fn flush(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        let ino_u64: u64 = ino.into();
        let fh_u64: u64 = fh.into();

        if let Err(errno) = self.flush_buffer(ino_u64, fh_u64) {
            reply.error(errno);
            return;
        }
        reply.ok();
    }

    fn fsync(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let ino_u64: u64 = ino.into();
        let fh_u64: u64 = fh.into();

        if let Err(errno) = self.flush_buffer(ino_u64, fh_u64) {
            reply.error(errno);
            return;
        }
        reply.ok();
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let parent_u64: u64 = parent.into();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let new_entry = NewFileEntry {
            parent_inode: parent_u64,
            name: name_str.to_owned(),
            is_dir: false,
            size: 0,
            permissions: 0o644,
            mtime: now,
            etag: None,
            content_hash: None,
            is_pinned: false,
            is_cached: true,
            sync_state: SyncState::PendingUpload,
        };

        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let inode = match cache.db().insert(&new_entry) {
            Ok(i) => i,
            Err(e) => {
                tracing::error!(parent = parent_u64, name = name_str, error = %e, "create: insert failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        // Write empty cache file
        if let Err(e) = self.rt.block_on(cache.put(inode, &bytes::Bytes::new())) {
            tracing::error!(inode, error = %e, "create: cache put failed");
            reply.error(Errno::EIO);
            return;
        }

        let fh = self.next_fh.fetch_add(1, Ordering::Relaxed);
        {
            let cache_dir = cache.cache_dir().to_path_buf();
            let wb = match WriteBuffer::new(&cache_dir, inode, fh, None) {
                Ok(wb) => wb,
                Err(e) => {
                    tracing::error!(inode, error = %e, "create: WriteBuffer creation failed");
                    reply.error(Errno::EIO);
                    return;
                }
            };
            let mut bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
            bufs.insert((inode, fh), wb);
        }

        let attr = self.to_file_attr(&FileEntry {
            inode,
            parent_inode: parent_u64,
            name: name_str.to_owned(),
            is_dir: false,
            size: 0,
            permissions: 0o644,
            mtime: now,
            etag: None,
            content_hash: None,
            is_pinned: false,
            is_cached: true,
            sync_state: SyncState::PendingUpload,
        });

        reply.created(
            &TTL,
            &attr,
            Generation(0),
            FileHandle(fh),
            FopenFlags::empty(),
        );
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        fh: Option<FileHandle>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let ino_u64: u64 = ino.into();
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());

        let mut entry = match cache.db().get_by_inode(ino_u64) {
            Ok(e) => e,
            Err(Error::InodeNotFound(_)) => {
                reply.error(Errno::ENOENT);
                return;
            }
            Err(e) => {
                tracing::error!(inode = ino_u64, error = %e, "setattr: get failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        // Handle truncate
        if let Some(new_size) = size {
            entry.size = new_size;

            // Also truncate the write buffer if one exists
            if let Some(fh_val) = fh {
                let fh_u64: u64 = fh_val.into();
                let mut bufs = self.write_bufs.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(buf) = bufs.get_mut(&(ino_u64, fh_u64))
                    && let Err(e) = buf.truncate(new_size)
                {
                    tracing::error!(inode = ino_u64, error = %e, "setattr: truncate buffer failed");
                    reply.error(Errno::EIO);
                    return;
                }
            }
        }

        // Handle mtime update
        if let Some(mtime_val) = mtime {
            entry.mtime = match mtime_val {
                fuser::TimeOrNow::SpecificTime(t) => {
                    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
                }
                fuser::TimeOrNow::Now => SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            };
        }

        let updated = NewFileEntry {
            parent_inode: entry.parent_inode,
            name: entry.name.clone(),
            is_dir: entry.is_dir,
            size: entry.size,
            permissions: entry.permissions,
            mtime: entry.mtime,
            etag: entry.etag.clone(),
            content_hash: entry.content_hash.clone(),
            is_pinned: entry.is_pinned,
            is_cached: entry.is_cached,
            sync_state: entry.sync_state,
        };

        if let Err(e) = cache.db().update_metadata(ino_u64, &updated) {
            tracing::error!(inode = ino_u64, error = %e, "setattr: update failed");
            reply.error(Errno::EIO);
            return;
        }

        reply.attr(&TTL, &self.to_file_attr(&entry));
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let parent_u64: u64 = parent.into();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let new_entry = NewFileEntry {
            parent_inode: parent_u64,
            name: name_str.to_owned(),
            is_dir: true,
            size: 0,
            permissions: 0o755,
            mtime: now,
            etag: None,
            content_hash: None,
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::PendingUpload,
        };

        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let inode = match cache.db().insert(&new_entry) {
            Ok(i) => i,
            Err(e) => {
                tracing::error!(parent = parent_u64, name = name_str, error = %e, "mkdir: insert failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        // Build remote path and send CreateDir to worker
        match self.build_remote_path(inode, &cache) {
            Ok(remote_path) => {
                if let Err(e) = self.upload_tx.send(UploadMessage::CreateDir(remote_path)) {
                    tracing::error!(error = %e, "upload channel disconnected");
                }
            }
            Err(e) => {
                tracing::error!(inode, error = %e, "mkdir: build_remote_path failed");
            }
        }

        let attr = self.to_file_attr(&FileEntry {
            inode,
            parent_inode: parent_u64,
            name: name_str.to_owned(),
            is_dir: true,
            size: 0,
            permissions: 0o755,
            mtime: now,
            etag: None,
            content_hash: None,
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::PendingUpload,
        });

        reply.entry(&TTL, &attr, Generation(0));
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let parent_u64: u64 = parent.into();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let entry = match cache.db().lookup(parent_u64, name_str) {
            Ok(e) => e,
            Err(Error::EntryNotFound(_, _)) => {
                reply.error(Errno::ENOENT);
                return;
            }
            Err(e) => {
                tracing::error!(parent = parent_u64, name = name_str, error = %e, "unlink: lookup failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        // Build remote path before deleting from DB
        let remote_path = match self.build_remote_path(entry.inode, &cache) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(inode = entry.inode, error = %e, "unlink: build_remote_path failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        // Remove from cache
        if let Err(e) = self.rt.block_on(cache.remove(entry.inode)) {
            tracing::error!(inode = entry.inode, error = %e, "unlink: cache remove failed");
        }

        // Delete from DB
        if let Err(e) = cache.db().delete(entry.inode) {
            tracing::error!(inode = entry.inode, error = %e, "unlink: db delete failed");
            reply.error(Errno::EIO);
            return;
        }

        // Send delete to upload worker
        if let Err(e) = self.upload_tx.send(UploadMessage::Delete(remote_path)) {
            tracing::error!(error = %e, "upload channel disconnected");
        }

        reply.ok();
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let parent_u64: u64 = parent.into();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let entry = match cache.db().lookup(parent_u64, name_str) {
            Ok(e) => e,
            Err(Error::EntryNotFound(_, _)) => {
                reply.error(Errno::ENOENT);
                return;
            }
            Err(e) => {
                tracing::error!(parent = parent_u64, name = name_str, error = %e, "rmdir: lookup failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        // Check directory is empty
        match cache.db().list_children(entry.inode) {
            Ok(children) if !children.is_empty() => {
                reply.error(Errno::ENOTEMPTY);
                return;
            }
            Err(e) => {
                tracing::error!(inode = entry.inode, error = %e, "rmdir: list_children failed");
                reply.error(Errno::EIO);
                return;
            }
            _ => {}
        }

        let remote_path = match self.build_remote_path(entry.inode, &cache) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(inode = entry.inode, error = %e, "rmdir: build_remote_path failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        if let Err(e) = cache.db().delete(entry.inode) {
            tracing::error!(inode = entry.inode, error = %e, "rmdir: db delete failed");
            reply.error(Errno::EIO);
            return;
        }

        if let Err(e) = self.upload_tx.send(UploadMessage::Delete(remote_path)) {
            tracing::error!(error = %e, "upload channel disconnected");
        }

        reply.ok();
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let parent_u64: u64 = parent.into();
        let newparent_u64: u64 = newparent.into();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };
        let newname_str = match newname.to_str() {
            Some(s) => s,
            None => {
                reply.error(Errno::EINVAL);
                return;
            }
        };

        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let entry = match cache.db().lookup(parent_u64, name_str) {
            Ok(e) => e,
            Err(Error::EntryNotFound(_, _)) => {
                reply.error(Errno::ENOENT);
                return;
            }
            Err(e) => {
                tracing::error!(parent = parent_u64, name = name_str, error = %e, "rename: lookup failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        let old_path = match self.build_remote_path(entry.inode, &cache) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(inode = entry.inode, error = %e, "rename: build old remote path failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        // Update DB
        if let Err(e) = cache
            .db()
            .move_entry(entry.inode, newparent_u64, newname_str)
        {
            tracing::error!(inode = entry.inode, error = %e, "rename: move_entry failed");
            reply.error(Errno::EIO);
            return;
        }

        // Mark as PendingUpload until remote move succeeds
        if let Err(e) = cache
            .db()
            .update_sync_state(entry.inode, SyncState::PendingUpload)
        {
            tracing::error!(inode = entry.inode, error = %e, "rename: update_sync_state failed");
        }

        // Build new remote path after the DB update
        let new_path = match self.build_remote_path(entry.inode, &cache) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(inode = entry.inode, error = %e, "rename: build new remote path failed");
                reply.error(Errno::EIO);
                return;
            }
        };

        if let Err(e) = self.upload_tx.send(UploadMessage::Move {
            inode: entry.inode,
            from: old_path,
            to: new_path,
        }) {
            tracing::error!(error = %e, "upload channel disconnected");
        }

        reply.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::backend::RemoteEntry;
    use crate::cache::CacheManager;
    use crate::db::models::{NewFileEntry, SyncState};
    use bytes::Bytes;

    struct DummyBackend;

    impl Backend for DummyBackend {
        async fn list_dir(&self, _: &str) -> crate::error::Result<Vec<RemoteEntry>> {
            Ok(vec![])
        }
        async fn get_metadata(&self, _: &str) -> crate::error::Result<RemoteEntry> {
            unimplemented!()
        }
        async fn download(&self, _: &str) -> crate::error::Result<Bytes> {
            Ok(Bytes::new())
        }
        async fn upload(&self, _: &str, _: Bytes) -> crate::error::Result<RemoteEntry> {
            unimplemented!()
        }
        async fn delete(&self, _: &str) -> crate::error::Result<()> {
            unimplemented!()
        }
        async fn move_entry(&self, _: &str, _: &str) -> crate::error::Result<()> {
            unimplemented!()
        }
        async fn create_dir(&self, _: &str) -> crate::error::Result<()> {
            unimplemented!()
        }
    }

    fn test_fs(tmp: &std::path::Path) -> MirageFs<DummyBackend> {
        let db = crate::db::Database::open_in_memory().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cache = rt
            .block_on(CacheManager::open(tmp.to_path_buf(), 1_000_000, db))
            .unwrap();
        let (tx, _rx) = std::sync::mpsc::channel();
        MirageFs::new(
            cache,
            Arc::new(DummyBackend),
            rt.handle().clone(),
            1000,
            1000,
            tx,
            Arc::new(std::sync::atomic::AtomicU8::new(0)),
        )
    }

    #[test]
    fn getattr_root() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = test_fs(tmp.path());
        let attr = fs.do_getattr(1).unwrap();
        assert_eq!(attr.ino, INodeNo(1));
        assert_eq!(attr.kind, FileType::Directory);
    }

    #[test]
    fn getattr_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = test_fs(tmp.path());
        assert!(fs.do_getattr(99999).is_err());
    }

    #[test]
    fn lookup_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = test_fs(tmp.path());
        assert!(fs.do_lookup(1, "nonexistent").is_err());
    }

    #[test]
    fn readdir_root_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = test_fs(tmp.path());
        let entries = fs.do_readdir(1).unwrap();
        // Should have . and .. only
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].2, ".");
        assert_eq!(entries[1].2, "..");
    }

    #[test]
    fn readdir_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = test_fs(tmp.path());
        assert!(fs.do_readdir(99999).is_err());
    }

    #[test]
    fn lookup_after_insert() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = test_fs(tmp.path());

        // Insert a file entry
        {
            let cache = fs.cache.lock().unwrap();
            cache
                .db()
                .insert(&NewFileEntry {
                    parent_inode: 1,
                    name: "test.txt".to_owned(),
                    is_dir: false,
                    size: 42,
                    permissions: 0o644,
                    mtime: 1000,
                    etag: None,
                    content_hash: None,
                    is_pinned: false,
                    is_cached: false,
                    sync_state: SyncState::Synced,
                })
                .unwrap();
        }

        let attr = fs.do_lookup(1, "test.txt").unwrap();
        assert_eq!(attr.kind, FileType::RegularFile);
        assert_eq!(attr.size, 42);
    }

    #[test]
    fn readdir_with_children() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = test_fs(tmp.path());

        {
            let cache = fs.cache.lock().unwrap();
            cache
                .db()
                .insert(&NewFileEntry {
                    parent_inode: 1,
                    name: "file1.txt".to_owned(),
                    is_dir: false,
                    size: 100,
                    permissions: 0o644,
                    mtime: 1000,
                    etag: None,
                    content_hash: None,
                    is_pinned: false,
                    is_cached: false,
                    sync_state: SyncState::Synced,
                })
                .unwrap();
            cache
                .db()
                .insert(&NewFileEntry {
                    parent_inode: 1,
                    name: "subdir".to_owned(),
                    is_dir: true,
                    size: 0,
                    permissions: 0o755,
                    mtime: 1000,
                    etag: None,
                    content_hash: None,
                    is_pinned: false,
                    is_cached: false,
                    sync_state: SyncState::Synced,
                })
                .unwrap();
        }

        let entries = fs.do_readdir(1).unwrap();
        assert_eq!(entries.len(), 4); // . + .. + file1.txt + subdir
        let names: Vec<&str> = entries.iter().map(|e| e.2.as_str()).collect();
        assert!(names.contains(&"file1.txt"));
        assert!(names.contains(&"subdir"));
    }
}
