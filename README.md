<p align="center">
  <img src="docs/mirage-icon.png" alt="Mirage" width="128">
</p>

# Mirage

Linux向けのクラウドファイル同期クライアント。
FUSE ベースの仮想ファイルシステムにより、ファイルをオンデマンドでダウンロードしながら、通常のディレクトリと同じように操作できます。

## Features

- **オンデマンドダウンロード** - ファイルを開いた時に初めてダウンロード。ストレージを節約
- **常時同期モード** - 重要なフォルダは常にローカルに保持（`mirage pin`）
- **オフライン対応** - キャッシュ済みファイルはオフラインでも読み書き可能
- **高速なディレクトリ操作** - ローカルDBからメタデータを即応答
- **LRUキャッシュ** - 設定した容量上限でキャッシュを自動管理
- **システムトレイ統合** - KDE / GNOME / XFCE 対応

## Installation

### Arch Linux

```bash
cd dist/
makepkg -si
```

## Usage

```bash
# 初回セットアップ（接続テスト + パスワードをキーリングに保存）
mirage setup

# デーモン起動
mirage daemon start

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

# 設定を確認・変更
mirage config list
mirage config get server_url
mirage config set server_url https://cloud.example.com

# デーモンのログを確認
mirage logs
mirage logs -f          # フォロー
mirage logs -n 50       # 直近50行
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

This project is licensed under the [MIT License](LICENSE).
