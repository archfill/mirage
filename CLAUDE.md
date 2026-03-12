# Mirage

## WHAT

Linux向けクラウドファイル同期クライアント。FUSE + ローカルSQLite DBにより、オンデマンドダウンロード型の仮想ファイルシステムを提供する。

- 言語: Rust
- 初期バックエンド: Nextcloud (WebDAV)
- ドキュメント: `docs/architecture.md`, `docs/features.md`

## Build & Test

```bash
cargo build                # ビルド
cargo test                 # テスト実行
cargo clippy               # リント
cargo fmt                  # フォーマット
cargo fmt -- --check       # フォーマットチェック（CI用）
```

## Rust Conventions

- `cargo clippy` の警告をゼロに保つ
- `cargo fmt` で統一フォーマット
- `unwrap()` / `expect()` は本番コードで使わない。`Result` / `Option` を適切にハンドリング
- unsafe は原則禁止。FUSE バインディング等でやむを得ない場合はコメントで理由を明記
- エラー型は `thiserror` で定義、呼び出し元への伝播は `?` 演算子
- ログは `tracing` crate を使用

## Architecture Rules

- FUSE コールバック内でネットワーク I/O を直接行わない（バックグラウンドワーカーに委任）
- メタデータは常にローカル DB から返す（readdir, getattr）
- バックエンド層は trait で抽象化し、コアロジックから分離
- キャッシュ管理とバックエンド通信は独立したモジュールにする

## Workflow

### Plan Mode (Shift+Tab)

- 3ステップ以上のタスクや設計判断を伴う場合は Plan mode で計画を先に提示
- 想定外の問題が発生したら即座に再計画
- 単純な修正（typo、1行変更）はスキップ可

### Subagent Strategy

- メインコンテキストを清潔に保つためサブエージェントを活用
- リサーチ・探索・並列分析はサブエージェントに委任
- 1サブエージェント = 1タスク

### Verification Before Done

- タスク完了前に必ず動作確認（テスト実行、ログ確認）
- `cargo test` / `cargo clippy` が通ることを確認
- 変更前後の挙動の差分を確認

### Self-Improvement

- ユーザーからの指摘後は `CLAUDE.md` にルールを追加して再発防止
