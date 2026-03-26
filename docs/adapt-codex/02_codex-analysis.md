# Codex CLI 分析

## 基本情報

- **リポジトリ**: github.com/openai/codex
- **言語**: Rust (95.7%)
- **認証**: ChatGPT アカウント or `CODEX_API_KEY` 環境変数
- **料金**: ChatGPT Plus/Pro/Business/Edu/Enterprise に含まれる

## 動作モード

### インタラクティブモード (`codex`)

- TUI ベースの対話インターフェース
- モデル選択可（GPT-5.4, GPT-5.3-Codex 等）
- 3つの承認モード: Suggest / Auto Edit / Full Auto
- スラッシュコマンド: `/resume`, `/fork`, `/compact`, `/model` 等
- 画像入力対応

### 非インタラクティブモード (`codex exec`)

- TUI なし、スクリプト/CI 向け
- `--json` フラグで JSONL ストリーム出力
- stdin からのパイプ入力対応: `echo "prompt" | codex exec -`
- `--output-schema` で JSON Schema バリデーション

## セッション永続化

- **自動 JSONL 保存**: 全セッションが JSONL として保存される
- **再開**: `codex resume` で過去セッションを選択・再開
- **フォーク**: `/fork` で既存セッションを分岐
- **コンパクション**: `/compact` でコンテキスト圧縮（最大7時間連続稼働）

## JSONL 出力フォーマット (`--json`)

`codex exec --json` 使用時のイベント型:

| イベント | 説明 |
|----------|------|
| `thread.started` | スレッド開始 |
| `turn.completed` | ターン完了 |
| `turn.failed` | ターン失敗 |
| `item.*` | エージェントメッセージ、推論、コマンド実行、ファイル変更、MCP ツール呼び出し、Web 検索 |

## Claude Code との比較

| 観点 | Claude Code | Codex CLI |
|------|-------------|-----------|
| 実行コマンド | `claude` | `codex` |
| 設定ディレクトリ | `~/.claude/` | `~/.codex/` |
| JSONL 保存場所 | `~/.claude/projects/-{path}/{id}.jsonl` | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` |
| JSONL スキーマ | `type: assistant/user/progress` | `type: session_meta/event_msg/response_item/turn_context` |
| 状態判定シグナル | `stop_reason: end_turn` | `payload.type: task_complete` |
| プロジェクト特定 | パスエンコーディング（ディレクトリ名） | session_meta の `cwd` フィールド |
| tmux 実行 | 問題なし | 問題なし（TUI を tmux 内で動かす） |

## 調査結果（2026-03-25）

### セッションファイル

- **保存場所**: `~/.codex/sessions/YYYY/MM/DD/rollout-{timestamp}-{uuid}.jsonl`
- **インタラクティブモードでも書き出す**: YES（TUI 実行時も同じ JSONL を生成）
- **プロジェクト特定**: 1行目の `session_meta` に `cwd` フィールドがある（Claude のパスエンコーディングとは異なる）
- **3096 件** のセッションファイルが存在（ヘビーユーザー）

### JSONL イベント一覧

| type | payload.type | payload.role | 意味 |
|------|-------------|-------------|------|
| `session_meta` | - | - | セッション開始メタデータ（cwd, model, cli_version 等） |
| `event_msg` | `task_started` | - | タスク開始（ユーザー入力後） |
| `event_msg` | `user_message` | - | ユーザーメッセージ |
| `event_msg` | `agent_message` | - | エージェントメッセージ |
| `event_msg` | `task_complete` | - | タスク完了 |
| `event_msg` | `token_count` | - | トークン使用量 |
| `response_item` | `message` | `user` | ユーザー入力 |
| `response_item` | `message` | `assistant` | アシスタント応答 |
| `response_item` | `message` | `developer` | システム/開発者メッセージ |
| `response_item` | `function_call` | - | ツール呼び出し |
| `response_item` | `function_call_output` | - | ツール実行結果 |
| `response_item` | `reasoning` | - | 推論ステップ |
| `turn_context` | - | - | ターンコンテキスト |

### chatmux ステータスマッピング

| Codex イベント | chatmux ステータス |
|---------------|-------------------|
| `task_complete` / `agent_message` / `token_count` | Replied |
| `message` (role=assistant) | Replied |
| `task_started` / `user_message` | Working |
| `function_call` / `function_call_output` / `reasoning` | Working |
| `turn_context` / `message` (role=developer) | Idle |

### ディレクトリ構造

```
~/.codex/
├── config.toml           # 設定
├── auth.json             # 認証
├── history.jsonl          # グローバル履歴
├── memories/             # メモリ
├── logs_*.sqlite         # ログDB
├── shell_snapshots/      # シェルスナップショット
└── sessions/             # セッション JSONL
    └── YYYY/MM/DD/
        └── rollout-{timestamp}-{uuid}.jsonl
```
