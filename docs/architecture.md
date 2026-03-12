# Architecture

## Overview

Mirage は FUSE + ローカル SQLite DB をコアとした仮想ファイル同期クライアント。
ファイルの実体はクラウドストレージに置きつつ、ローカルのディレクトリとして透過的に操作できる。

## System Architecture

```
[Cloud Storage Server]
       ↕ WebDAV (background sync)
[Local SQLite DB] ← metadata (filename, size, hash, ETag)
       ↕
[FUSE filesystem] → virtual file tree presented to user
       ↕
[Local cache]     ← actual files (downloaded on demand, LRU eviction)
```

## Why FUSE + Local DB

### Adopted: FUSE + Local DB

- FUSE はユーザー空間で動作し、カーネル更新の影響を受けない
- すべてのシステムコール（getattr, open, read, readdir, write）を自分で制御できる
- ローカル DB にメタデータを持つことで、readdir() が即応答・オフラインでもツリー表示可能
- FUSE のオーバーヘッド（マイクロ秒）は WebDAV のネットワーク遅延（ミリ秒〜秒）に対して無視できる

### Rejected Approaches

| Approach             | Reason                                                                                                                  |
| -------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| Overlayfs + fanotify | Stub files have 0 byte size, causing apps to misidentify files. Hook mechanism for open → download wait is also complex |
| Kernel module        | Tied to kernel version, high build/maintenance cost. Upstream merge is unrealistic on a multi-year timescale            |

## Tech Stack

| Component   | Choice                                               |
| ----------- | ---------------------------------------------------- |
| Language    | Rust (memory safety, single binary)                  |
| FUSE        | `fuser` crate                                        |
| Metadata DB | SQLite (`rusqlite`)                                  |
| WebDAV      | `reqwest` + tokio (async)                            |
| Cache       | LRU eviction, configurable capacity limit            |
| Desktop     | D-Bus (Nautilus/Dolphin status via SNI/AppIndicator) |

## Key Design Principles

- **readdir() responds instantly from local DB** - directory operations never hang
- **getattr() returns correct file size from local DB** - transparent to applications
- **open()/read() triggers download** - delegated to background worker, not blocking FUSE
- **write() caches locally, uploads in background**
- **Metadata prefetching strategy** determines user experience
- **Download priority control** - user-opened files get highest priority
- **Graceful offline handling** - serve cached files, queue writes for later upload
- **Nextcloud notify_push support** - real-time metadata sync

## Backend Abstraction

Nextcloud-specific parts are limited to WebDAV communication and notify_push. The backend layer is abstracted as a Rust trait, enabling multi-provider support.

```
[FUSE + Local DB + Cache] ← core (shared)
       ↕
[Backend trait]
       ↕
  ┌────┼────┬────────┐
  Nextcloud  Google   OneDrive  S3
  (WebDAV)   Drive    (Graph    (S3 API)
             (REST)    API)
```

### Backend Trait Interface

- List files (metadata)
- Download / Upload files
- Receive change notifications (if supported)
- Delete / Rename

**Strategy: Build Nextcloud-only first, then extract the backend trait once it stabilizes.** Premature generalization risks never shipping.
