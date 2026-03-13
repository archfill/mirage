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
- [x] Cache file storage ({cache_dir}/{inode})
- [x] Startup rebuild from filesystem mtime
- [ ] Cache status command (`mirage status`)

## Filesystem

- [ ] FUSE mount (present cloud file tree at a specified directory)
- [ ] Instant file metadata display (size, modified time, permissions)
- [ ] On-demand download (fetch file content only when opened)
- [ ] Local write → background upload
- [ ] Directory create / delete / rename

## Sync Modes

- [ ] On-demand (default): download on access
- [ ] Always local: keep specified folders/files synced at all times
- [ ] Per-folder / per-file mode switching

## Offline Support

- [ ] Read/write cached files while offline
- [ ] Queue writes during offline, auto-upload on reconnect
- [ ] Conflict detection and notification

## CLI

- [ ] `mirage mount <mountpoint>` - mount
- [ ] `mirage unmount` - unmount
- [ ] `mirage status` - show sync state and cache usage
- [ ] `mirage pin <path>` - mark as always local
- [ ] `mirage unpin <path>` - revert to on-demand
- [ ] `mirage config` - configure server URL, auth, cache limit, etc.

## System Tray

- [ ] Tray icon showing sync status
- [ ] Right-click menu (status / pin / unpin / settings / unmount)
- [ ] Sync progress display
- [ ] Error and conflict notifications
- [ ] KDE (SNI) / GNOME (AppIndicator) / XFCE support

## Daemon

- [ ] Run as systemd service
- [ ] Auto-mount on login
- [ ] Network state monitoring and auto-reconnect
