# Features

## Project Foundation

- [x] Project documentation (README, architecture, features)
- [x] Rust project skeleton (Cargo.toml, module structure)
- [x] CLI entrypoint with clap (mount, unmount, status, pin, unpin, config)
- [x] Error type definition (thiserror)
- [x] Config struct (serde + toml)
- [x] Logging setup (tracing + tracing-subscriber)
- [x] CI pipeline (GitHub Actions: fmt, clippy, build, test)
- [x] Stub modules (fuse, db, cache, backend/nextcloud)
- [x] `Config::load()` — `~/.config/mirage/config.toml` 読み込み

## Metadata DB

- [x] SQLite metadata store (inode-based file tree, WAL mode)
- [x] CRUD operations (insert, lookup, list_children, update, delete)
- [x] Sync state tracking (synced, pending_upload, pending_download, conflict)
- [x] Pin / cache flag management

## Backend

- [x] Backend trait abstraction (list_dir, get_metadata, download, upload, delete, move, mkdir)
- [x] RemoteEntry → NewFileEntry conversion
- [x] Nextcloud WebDAV client (PROPFIND, GET, PUT, DELETE, MOVE, MKCOL)
- [x] WebDAV XML parser (multistatus, resourcetype, checksums)
- [x] Integration tests with wiremock

## Sync Engine

- [x] Initial full metadata sync (server → local DB)
- [x] Incremental sync with ETag-based change detection
- [x] Reconciler (insert new, update changed, delete removed entries)
- [x] Recursive directory traversal
- [x] Sync report (counts for inserted, updated, deleted, errors)

## Cache Management

- [x] LRU eviction (configurable capacity limit)
- [x] Pinned files excluded from eviction
- [x] PendingUpload files excluded from eviction
- [x] Cache file storage ({cache_dir}/{inode})
- [x] Startup rebuild from filesystem mtime

## Filesystem

- [x] FUSE mount (present cloud file tree at a specified directory)
- [x] Instant file metadata display (size, modified time, permissions)
- [x] On-demand download (fetch file content only when opened)
- [x] Local write → background upload (write/create/flush/fsync/release)
- [x] Directory create / delete / rename (mkdir/rmdir/unlink/rename)
- [x] setattr (truncate, mtime update)

## Large File / Non-blocking / Graceful Shutdown

- [x] Disk-based write buffer (avoids OOM for large files)
- [x] Non-blocking download (Mutex released during file download)
- [x] Graceful shutdown (drain pending uploads on Ctrl+C)
- [x] Orphan temp file cleanup on startup

## Conflict Detection & Retry

- [x] SyncState::Conflict for local-pending-upload vs remote-changed
- [x] Reconciler detects conflicts and sets Conflict state
- [x] Exponential backoff for upload retries
- [x] Transient vs permanent error classification
- [x] `mirage conflicts` command

## Sync Modes

- [x] On-demand (default): download on access
- [x] Always local: keep specified folders/files synced at all times
- [x] Per-folder / per-file mode switching

## Offline Support

- [x] Read/write cached files while offline
- [x] Queue writes during offline, auto-upload on reconnect
- [x] Conflict detection and notification
- [x] Network state monitoring (lock-free AtomicU8)
- [x] Offline-aware FUSE open() returns EHOSTUNREACH for uncached files
- [x] Conflict resolution (`mirage resolve <path> keep-local|keep-remote|keep-both`)
- [x] Recursive pin/unpin (`--recursive` flag)

## CLI

- [x] `mirage mount <mountpoint>` - mount (read-write, Phase 2A)
- [x] `mirage unmount` - unmount
- [x] `mirage status` - show sync state and cache usage
- [x] `mirage pin <path> [--recursive]` - mark as always local
- [x] `mirage unpin <path> [--recursive]` - revert to on-demand
- [x] `mirage config init` - generate template config file
- [x] `mirage config path` - show config file path
- [x] `mirage conflicts` - list files in conflict state
- [x] `mirage resolve <path> <keep-local|keep-remote|keep-both>` - resolve conflict
- [x] `mirage daemon start|stop|status` - daemon management
- [x] `mirage tray` - launch system tray application

## System Tray

- [x] Tray icon showing sync status (ksni/SNI)
- [x] Right-click menu (status / quit)
- [x] Sync progress display (IPC GetProgress + tray menu)
- [x] Error and conflict notifications (notify-rust)
- [x] KDE (SNI) / XFCE / Sway / i3 support (GNOME requires AppIndicator extension)

## Daemon

- [x] Run as systemd service
- [x] Auto-mount on login (mount point auto-creation)
- [x] Network state monitoring and auto-reconnect
- [x] journald log integration for daemon mode
