# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/),
and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- FUSE read-write filesystem with disk-backed write buffers
- Background upload worker with exponential backoff retry
- Sync engine with recursive directory reconciliation
- LRU cache manager with pinning support (O(1) via `lru` crate)
- Nextcloud WebDAV backend (PROPFIND, GET, PUT, DELETE, MOVE, MKCOL)
- SQLite metadata database with WAL mode
- Offline resilience with network state monitor
- Conflict resolution strategies (KeepLocal, KeepRemote, KeepBoth)
- Daemon mode with PID file locking and systemd service support
- System tray integration (KDE/GNOME/XFCE) via D-Bus
- IPC protocol over Unix domain socket (status, progress, quit)
- CLI commands: mount, unmount, status, pin, unpin, daemon, config
- `.mirageignore` support for filtering files during sync
- `remote_base_path` config for scoping sync to specific subfolders
- Sync progress tracking with phase/file/byte counters

### Fixed

- Cache file leak on sync Delete (#1)
- Silent failure of Delete/Move remote operations (#2)
- OOM risk when flushing large files (#3)
- Password stored as plaintext String instead of SecretString (#4)
- IPC socket world-readable by default (#5)
- Upload channel send failures silently ignored (#6)
- O(n) LRU promote operation (#7)
- PID file truncated before flock acquisition (#8)
- No rollback on rename remote failure (#9)

## [0.0.0] - 2026-03-13

- Initial project setup with module skeleton
