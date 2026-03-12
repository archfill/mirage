# Mirage

![Status](https://img.shields.io/badge/status-WIP-yellow)

> **Note:** This project is in early development and not yet functional. APIs, architecture, and features are subject to change.

Linux向けのクラウドファイル同期クライアント。
FUSE ベースの仮想ファイルシステムにより、ファイルをオンデマンドでダウンロードしながら、通常のディレクトリと同じように操作できます。

## Features

- **オンデマンドダウンロード** - ファイルを開いた時に初めてダウンロード。ストレージを節約
- **常時同期モード** - 重要なフォルダは常にローカルに保持（`mirage pin`）
- **オフライン対応** - キャッシュ済みファイルはオフラインでも読み書き可能
- **高速なディレクトリ操作** - ローカルDBからメタデータを即応答
- **LRUキャッシュ** - 設定した容量上限でキャッシュを自動管理
- **システムトレイ統合** - KDE / GNOME / XFCE 対応

## Usage

```bash
# マウント
mirage mount ~/cloud

# ファイルを常にローカルに保持
mirage pin ~/cloud/important

# オンデマンドに戻す
mirage unpin ~/cloud/important

# 同期状態・キャッシュ使用量を確認
mirage status

# アンマウント
mirage unmount
```

## Architecture

```
[Cloud Storage Server]
       ↕ WebDAV (background sync)
[Local SQLite DB] ← metadata (filename, size, hash, ETag)
       ↕
[FUSE filesystem] → virtual file tree presented to user
       ↕
[Local cache]     ← actual files (downloaded on demand, LRU eviction)
```

## Tech Stack

- **Rust** - Memory safety, single binary distribution
- **fuser** - FUSE filesystem implementation
- **SQLite (rusqlite)** - Local metadata database
- **reqwest + tokio** - Async WebDAV communication
- **D-Bus** - Desktop environment integration

## Supported Backends

Currently targeting **Nextcloud (WebDAV)**. The backend layer is designed as an abstract trait, enabling future support for other cloud storage providers (Google Drive, OneDrive, S3, etc.).

## Documentation

- [Architecture](docs/architecture.md) - System design and technical decisions
- [Features](docs/features.md) - Feature list and specifications

## License

TBD
