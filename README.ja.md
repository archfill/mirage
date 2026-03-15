<p align="center">
  <img src="docs/mirage-icon.png" alt="Mirage" width="128">
</p>

# Mirage

[![CI](https://github.com/archfill/mirage/actions/workflows/ci.yml/badge.svg)](https://github.com/archfill/mirage/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform: Linux](https://img.shields.io/badge/Platform-Linux-yellow.svg)](https://github.com/archfill/mirage)

[![English](https://img.shields.io/badge/lang-English-red.svg)](README.md)

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

- **Rust** - メモリ安全性、シングルバイナリ配布
- **fuser** - FUSE ファイルシステム実装
- **SQLite (rusqlite)** - ローカルメタデータDB
- **reqwest + tokio** - 非同期 WebDAV 通信
- **D-Bus** - デスクトップ環境との統合

## Supported Backends

現在は **Nextcloud (WebDAV)** をメインターゲットとしています。バックエンド層は抽象トレイトとして設計されており、将来的に他のクラウドストレージ（Google Drive、OneDrive、S3 など）への対応も可能です。

## Documentation

- [Architecture](docs/architecture.md) - システム設計と技術的な決定事項
- [Features](docs/features.md) - 機能一覧と仕様

## License

このプロジェクトは [MIT License](LICENSE) のもとで公開されています。
