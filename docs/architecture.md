# Architecture

## Overview

Mirage は FUSE + ローカル SQLite DB をコアとした仮想ファイル同期クライアント。
ファイルの実体はクラウドストレージに置きつつ、ローカルのディレクトリとして透過的に操作できる。

## System Architecture

```
                        [Cloud Storage Server]
                               ↕ WebDAV
              ┌────────────────┼────────────────┐
         [Sync Engine]   [Upload Worker]   [Backend::ping()]
              ↕                ↕                ↕
         [Local SQLite DB] ← metadata    [NetworkMonitor]
              ↕                                 ↕
         [FUSE filesystem] → virtual file tree presented to user
              ↕
         [Local cache] ← LRU eviction, on-demand download
              ↕
    ┌─────────┼──────────┐
[IPC Server]  │    [PID Lock]
    ↕         │
[System Tray] │ ← ksni (StatusNotifierItem)
    ↕         │
[Desktop      │ ← notify-rust (freedesktop)
 Notifications]
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

| Component    | Choice                                                |
| ------------ | ----------------------------------------------------- |
| Language     | Rust (memory safety, single binary)                   |
| FUSE         | `fuser` crate                                         |
| Metadata DB  | SQLite (`rusqlite`, WAL mode)                         |
| WebDAV       | `reqwest` + tokio (async)                             |
| Cache        | LRU eviction, configurable capacity limit             |
| System Tray  | `ksni` (StatusNotifierItem / D-Bus)                   |
| Notification | `notify-rust` (freedesktop Notifications)             |
| Logging      | `tracing` + `tracing-subscriber` / `tracing-journald` |
| IPC          | Unix domain socket + JSON (`serde_json`)              |
| Process Lock | `flock(2)` + PID file                                 |

## Key Design Principles

- **readdir() responds instantly from local DB** - directory operations never hang
- **getattr() returns correct file size from local DB** - transparent to applications
- **open()/read() triggers download** - `rt.block_on(backend.download())` でオンデマンド取得。Mutex を解放してからダウンロードすることで他の FUSE 操作をブロックしない
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

## FUSE Implementation Notes

### Phase 1: Read-only mount (実装済み)

| Callback  | 動作                                                         |
| --------- | ------------------------------------------------------------ |
| `getattr` | DB `get_by_inode` → `FileAttr` 返却                          |
| `lookup`  | DB `lookup(parent, name)` → `ReplyEntry`                     |
| `readdir` | DB `list_children` + `.` / `..` エントリ追加                 |
| `open`    | キャッシュ済み → 即応答。未キャッシュ → download → cache.put |
| `read`    | `cache.read(ino)` → スライスして返却                         |
| `release` | No-op                                                        |

### スレッド安全性

`fuser` の `Filesystem` trait は `Send + Sync + 'static` を要求する。
`rusqlite::Connection` は `!Sync` のため、`CacheManager` を `Mutex<CacheManager>` でラップして `MirageFs` に保持している。

### Phase 2A: 書き込み対応 (実装済み)

| Callback        | 動作                                                          |
| --------------- | ------------------------------------------------------------- |
| `write`         | in-memory バッファ `(inode, fh)` に offset 書き込み           |
| `create`        | DB insert (PendingUpload) → 空キャッシュ作成 → バッファ初期化 |
| `setattr`       | truncate (size変更) / mtime 更新 → DB 更新                    |
| `mkdir`         | DB insert (is_dir, PendingUpload) → ワーカーに CreateDir 送信 |
| `unlink`        | cache.remove → ワーカーに Delete 送信 → db.delete             |
| `rmdir`         | children 確認 → ENOTEMPTY or unlink 同等フロー                |
| `rename`        | DB move_entry → ワーカーに Move 送信                          |
| `flush`/`fsync` | バッファを cache.put → DB 更新 (size, mtime, PendingUpload)   |
| `release`       | バッファ flush → upload_tx に Upload 通知                     |

### Upload Worker

専用スレッド上で動作するバックグラウンドアップロードワーカー。
FUSE コールバックから `mpsc` チャネル経由でメッセージを受信し、Backend trait のメソッドを呼び出す。

- `Upload(inode)`: キャッシュファイル読み込み → remote path 構築 → `backend.upload()` → DB を Synced に更新
- `Delete(path)`: `backend.delete()` 呼び出し
- `Move { from, to }`: `backend.move_entry()` 呼び出し
- `CreateDir(path)`: `backend.create_dir()` 呼び出し
- 30秒タイムアウト時: `db.get_by_sync_state(PendingUpload)` でリトライ
- コンフリクト戦略: PendingUpload + リモート変更時に `Conflict` 状態に遷移（Phase 2C 実装済み）

### Phase 2B: 大ファイル対応 + 非ブロッキング + 安全停止 (実装済み)

| 機能                       | 動作                                                                                        |
| -------------------------- | ------------------------------------------------------------------------------------------- |
| ディスクベース書き込み     | `WriteBuffer` が `{cache_dir}/.write_{inode}_{fh}` に一時ファイルを作成、finalize で rename |
| 非ブロッキングダウンロード | `open()` が Mutex を解放してからダウンロード → 他の FUSE 操作をブロックしない               |
| グレースフルシャットダウン | Ctrl+C → `UploadMessage::Shutdown` → チャネル drain → `retry_pending()` → 終了              |
| orphan cleanup             | `CacheManager::open()` で `.write_*` 残骸を自動削除                                         |
| `mirage config init`       | テンプレート config.toml を生成                                                             |

### Phase 2C: コンフリクト検出 + Pin 自動 DL + リトライ改善 (実装済み)

| 機能                   | 動作                                                                 |
| ---------------------- | -------------------------------------------------------------------- |
| コンフリクト検出       | PendingUpload + リモート ETag 変更 → `SyncState::Conflict` に遷移    |
| `mirage conflicts`     | Conflict 状態のファイル一覧表示                                      |
| Pin 自動ダウンロード   | `full_sync()` 末尾で pin 済み未キャッシュファイルを自動取得          |
| 指数バックオフリトライ | `retry_base_secs * 2^n`（最大 `retry_max_secs`）で段階的にバックオフ |
| エラー分類             | `Error::is_transient()` で一時的/永続的エラーを区別                  |

### Phase 2D: オフライン耐性 + 再帰 Pin + コンフリクト解決 (実装済み)

| 機能              | 動作                                                                         |
| ----------------- | ---------------------------------------------------------------------------- |
| NetworkMonitor    | `AtomicU8` ベースのロックフリー状態管理、sync/upload/FUSE スレッドで共有     |
| オフライン sync   | sync ループ先頭で `ping()` → 失敗時はスキップ、成功復帰で自動再開            |
| オフライン upload | `retry_pending()` がオフライン時にスキップ、transient エラーで自動遷移       |
| オフライン FUSE   | 未キャッシュ + Offline → `EHOSTUNREACH`、キャッシュ済みファイルは正常応答    |
| Backend::ping()   | Depth:0 PROPFIND（3秒タイムアウト）で軽量到達確認                            |
| 再帰 Pin/Unpin    | `set_pinned_recursive()` で子孫を一括 pin/unpin、CLI に `--recursive` フラグ |
| コンフリクト解決  | `mirage resolve` コマンドで keep-local / keep-remote / keep-both の3戦略     |

## Phase 3: Sync Modes + Daemon (実装済み)

| 機能               | 動作                                                                     |
| ------------------ | ------------------------------------------------------------------------ |
| Always-local sync  | `config.always_local_paths` にマッチするファイルを sync 挿入時に自動 pin |
| PID ファイルロック | `flock(2)` + PID ファイルで多重起動防止                                  |
| systemd サービス   | `contrib/mirage.service` で Type=simple として管理                       |
| daemon CLI         | `start` / `stop` (SIGTERM) / `status` (flock probe)                      |

## Phase 4: Daemon 仕上げ + System Tray (実装済み)

| 機能                     | 動作                                                                          |
| ------------------------ | ----------------------------------------------------------------------------- |
| journald ログ            | daemon 起動時に `tracing-journald` layer を使用、`journalctl --user` で確認可 |
| マウントポイント自動作成 | `run_mount()` で `create_dir_all()` を FUSE マウント前に実行                  |
| IPC (Unix socket)        | `$XDG_RUNTIME_DIR/mirage.sock` で daemon ↔ tray 間 JSON 通信                  |
| System Tray              | `ksni` crate (StatusNotifierItem) でトレイアイコン + ステータスメニュー       |
| デスクトップ通知         | `notify-rust` で conflict 検出時に freedesktop 通知送信                       |
