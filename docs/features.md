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

## Cache Management

- [ ] LRU eviction (configurable capacity limit)
- [ ] Pinned files excluded from eviction
- [ ] Cache status command (`mirage status`)

## Offline Support

- [ ] Read/write cached files while offline
- [ ] Queue writes during offline, auto-upload on reconnect
- [ ] Conflict detection and notification

## Metadata Sync

- [ ] Initial: full metadata fetch from server to local DB
- [ ] Incremental: sync changes via ETag / notify_push
- [ ] readdir() served from local DB (no network required)

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
