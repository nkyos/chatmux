# Codex 対応設計: 概要

## 背景

chatmux は現在 Claude Code 専用。Phase 3 ロードマップに「マルチエージェント対応」があり、
その第一弾として OpenAI Codex CLI への対応を設計する。

## ゴール

1. **Codex CLI をセッションとして管理できる** — Claude Code と同様の体験
2. **汎用的なアダプタ層を導入する** — 将来の他エージェント対応の基盤
3. **既存の Claude Code 体験を壊さない** — 段階的に導入可能

## スコープ

### In

- Agent トレイト（アダプタインターフェース）の設計・実装
- Claude Code アダプタ（既存ロジックのリファクタ）
- Codex CLI アダプタの実装
- セッション作成時のエージェント選択 UI
- サイドバーでのエージェント種別表示

### Out（将来）

- Aider, Cline 等の他エージェント対応
- エージェント間の連携・コンテキスト共有
- プラグインシステム

## 現状のハードコード箇所

chatmux で Claude Code に依存している箇所の一覧:

| ファイル | 行 | 内容 |
|----------|-----|------|
| `src/tmux/client.rs` | L34 | `"claude"` コマンド固定 |
| `src/jsonl.rs` | L18-20 | `~/.claude/projects/` パス固定 |
| `src/jsonl.rs` | L10-12 | Claude 固有のパスエンコーディング |
| `src/jsonl.rs` | L109-128 | Claude JSONL スキーマ固有のパース |
| `src/session/model.rs` | 全体 | エージェント種別フィールドなし |

## 設計方針

**Method D（ハイブリッド）を継続**:
- セッション実行: tmux ラッパー（エージェントのコマンドを起動）
- 状態検知: エージェント固有のJSONL/出力を監視

Codex も Claude Code も CLI ツールであり、tmux 内で実行する基本構造は同じ。
違いは「コマンド名」「設定ディレクトリ」「JSONL フォーマット」の3点。
