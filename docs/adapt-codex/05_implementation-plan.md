# 実装計画

## 前提

- Phase 2（磨き込み）の機能とは独立して進められる
- 既存の Claude Code 体験を壊さないよう段階的に導入

## Step 0: 事前検証（コード変更なし）

Codex CLI の動作確認。設計の前提を検証する。

- [ ] Codex を tmux 内で起動して動作確認
- [ ] tmux resize-window への追従確認
- [ ] send-keys での入力転送確認
- [ ] セッションファイルの保存場所と形式を特定
- [ ] capture-pane 出力でのプロンプト文字パターンを記録

**成果物**: `02_codex-analysis.md` の「要調査事項」を埋める

## Step 1: Agent トレイト導入（リファクタのみ）

既存コードを Agent トレイトベースに書き換える。
**この時点では Claude Code のみ対応**。動作に変更なし。

### 1-1: Agent トレイト定義

```
src/agent/
├── mod.rs          // Agent トレイト、AgentKind、AgentRegistry
├── claude_code.rs  // ClaudeCodeAgent
└── codex.rs        // CodexAgent（スタブ）
```

### 1-2: Session モデルに AgentKind 追加

```rust
// src/session/model.rs
pub struct Session {
    pub agent_kind: AgentKind,  // 追加
    // ...
}
```

永続化（sessions.json）にも agent_kind を含める。
デフォルト値は `ClaudeCode`（後方互換性）。

### 1-3: TmuxClient のコマンド動的化

```rust
// "claude" ハードコード → command 引数
pub fn new_session(&self, name: &str, cwd: &str,
                   command: &str, args: &[String],
                   width: u16, height: u16) -> Result<()>
```

### 1-4: JSONL ロジックを ClaudeCodeAgent に移動

`src/jsonl.rs` の中身を `src/agent/claude_code.rs` に移動。
`src/jsonl.rs` は共通ユーティリティ（ファイル読み込み等）のみ残す。

### 1-5: SessionManager / App の Agent 対応

- SessionManager::create() が Agent を受け取る
- poll_session_statuses() が AgentRegistry 経由でアダプタを取得

**検証**: この時点で `cargo test` + 手動テストで既存動作が維持されていることを確認。

## Step 2: エージェント選択 UI

### 2-1: ProjectPicker にエージェント選択を追加

プロジェクト選択後、エージェントを選択するステップを追加:

```
[プロジェクト選択] → [エージェント選択] → [セッション作成]
                     ┌──────────────┐
                     │ 🟣 Claude Code │
                     │ 🟢 Codex       │
                     └──────────────┘
```

- インストール済みのエージェントのみ表示（`which` で検出）
- エージェントが1つしかなければスキップ

### 2-2: サイドバーにエージェントアイコン表示

```
⏳ 🟣 chatmux     3m ago
🔴 🟢 my-api      1m ago
💤 🟣 frontend    15m ago
```

## Step 3: Codex アダプタ実装

### 3-1: 画面スクレイピングベースの状態検知

Step 0 の検証結果を元に、capture-pane 出力からの状態推定ロジックを実装。

```rust
fn detect_status_from_screen(output: &str) -> SessionStatus {
    // Codex 固有のプロンプトパターンをマッチ
}
```

### 3-2: JSONL ベースの状態検知（保存場所判明後）

Codex のセッション JSONL を監視するロジックを実装。

### 3-3: Codex プロジェクト発見

Codex の過去セッションからプロジェクトを発見する機能。
（初期はスキップ可 — DirectoryBrowser で十分）

## Step 4: 磨き込み

- エージェント固有の設定（config.toml）
- エージェント別のキーバインドやショートカット
- エラーハンドリング（Codex 未インストール時のフォールバック）

## 依存関係

```
Step 0 ──→ Step 1 ──→ Step 2
              │
              └──→ Step 3 ──→ Step 4
```

Step 2 と Step 3 は Step 1 完了後に並行して進められる。

## 工数見積もり

| ステップ | 内容 | 規模感 |
|----------|------|--------|
| Step 0 | 事前検証 | 手動作業 30分 |
| Step 1 | Agent トレイト導入 | 中（リファクタ主体、新ロジックなし） |
| Step 2 | エージェント選択 UI | 小〜中 |
| Step 3 | Codex アダプタ | 中（Step 0 の結果次第） |
| Step 4 | 磨き込み | 小 |
