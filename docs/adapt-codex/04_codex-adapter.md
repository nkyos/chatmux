# Codex アダプタ実装詳細

## アプローチ選択

### 案 A: tmux + インタラクティブモード（推奨）

```
tmux new-session -d -s chatmux-s0 -c /project codex
```

Claude Code と同じアプローチ。`codex` の TUI を tmux 内で動かし、
capture-pane で画面を取得、send-keys で入力を転送する。

**メリット**:
- 既存アーキテクチャと統一
- Codex の全機能（スラッシュコマンド、モデル選択）がそのまま使える
- ユーザーにとっての操作感が Claude Code と同じ

**デメリット**:
- 状態検知に JSONL ファイルの場所特定が必要
- Codex TUI の表示が tmux のリサイズに追従するか検証が必要

### 案 B: codex exec --json（非インタラクティブ）

標準出力の JSONL をパースして状態管理する。

**メリット**:
- JSONL フォーマットが公式ドキュメントに記載
- ファイル探索が不要（stdout を直接読める）

**デメリット**:
- 対話的な操作（追加指示、承認）ができない
- chatmux の UX 哲学（「対話型チャット」）と合わない
- resume/fork 等の機能が使えない

### 結論: 案 A を採用

chatmux は「AI エージェントとの並行対話」がコアバリュー。
非インタラクティブモードはこのバリューと矛盾する。

## 状態検知戦略

### Phase 1: 画面スクレイピング（即座に実装可能）

Codex のセッションファイル場所が確定するまでの暫定策。

```rust
impl Agent for CodexAgent {
    fn detect_status(&self, _path: &Path) -> Option<DetectedStatus> {
        None  // フォールバック: 画面スクレイピングに委譲
    }
}
```

capture-pane 出力から状態を推定:
- Codex のプロンプト文字（入力待ち）が末尾 → Replied
- スピナーや処理中表示 → Working
- 何も動いていない → Idle

### Phase 2: JSONL 監視（Codex の保存場所特定後）

実際に `codex` を実行して確認が必要な項目:

```bash
# セッションファイルの場所を特定
find ~/.codex -name "*.jsonl" 2>/dev/null
find ~/.config/codex -name "*.jsonl" 2>/dev/null

# または codex のソースコードから
# github.com/openai/codex で session storage を検索
```

特定後、Claude Code と同様の tail + parse ロジックを実装。

## Codex JSONL スキーマ（codex exec --json 参考）

```jsonl
{"type": "thread.started", "thread_id": "..."}
{"type": "item.message", "role": "assistant", "content": "..."}
{"type": "item.command_execution", "command": "...", "exit_code": 0}
{"type": "item.file_change", "path": "...", "action": "modify"}
{"type": "item.mcp_tool_call", "tool": "...", "result": "..."}
{"type": "turn.completed", "usage": {...}}
{"type": "turn.failed", "error": "..."}
```

**状態マッピング**:

| Codex イベント | chatmux ステータス |
|---------------|-------------------|
| `turn.completed` | Replied |
| `turn.failed` | Replied (エラー表示) |
| `item.command_execution` (実行中) | Working |
| `item.*` (ストリーミング中) | Working |
| `thread.started` 後、入力待ち | Idle |

## 設定

```toml
# ~/.config/chatmux/config.toml

[agents.codex]
command = "codex"          # デフォルト
# args = ["--model", "gpt-5.4"]  # オプション引数
```

## プロジェクト一覧

Codex の過去プロジェクト発見:

```rust
fn discover_projects(&self) -> Vec<ProjectInfo> {
    // 案1: Codex のセッションディレクトリをスキャン
    // 案2: `codex resume` の出力をパース（非推奨: 対話的コマンド）
    // 案3: Claude のプロジェクト一覧と共有（cwd ベースなので共通化可能）
}
```

**現実的な案**: プロジェクト選択は cwd ベースの DirectoryBrowser を使う。
Codex 固有の過去セッション一覧は将来対応。

## 検証タスク

実装前に手動で確認すべき項目:

- [ ] `codex` コマンドが tmux 内で正常に動作するか
- [ ] tmux resize-window に Codex TUI が追従するか
- [ ] Codex のセッションファイル保存場所
- [ ] インタラクティブモードでの JSONL 書き出し有無
- [ ] send-keys での入力転送がスムーズか（特殊キーの扱い）
