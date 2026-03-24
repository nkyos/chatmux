# 技術設計

## 技術スタック

| 項目 | 選定 | 理由 |
|------|------|------|
| 言語 | Rust | シングルバイナリ、パフォーマンス、かっこいい |
| TUI | ratatui + crossterm | 成熟した TUI フレームワーク |
| 非同期 | tokio | ファイル監視・tmux 操作の非同期処理 |
| tmux 操作 | std::process::Command ラッパー | 直接コマンド実行 |
| ファイル監視 | notify crate | ファイルシステムイベント監視 |
| OS 通知 | notify-rust | macOS/Linux 対応 |
| 設定 | TOML (toml crate) | Rust エコシステムの標準 |
| シリアライズ | serde + serde_json | JSONL パース |

## アーキテクチャ

```
┌──────────────────────────────────────────────┐
│                 Ratatui App               │
│                                              │
│  ┌─ Sidebar ──────┐  ┌─ TerminalView ─────┐ │
│  │ SessionList     │  │ capture-pane 出力   │ │
│  │ component       │  │ (定期リフレッシュ)   │ │
│  └────────┬───────┘  └────────┬────────────┘ │
│           │                   │              │
│  ┌────────▼───────────────────▼────────────┐ │
│  │           SessionManager                 │ │
│  │  - セッション CRUD                        │ │
│  │  - 状態管理 (working/replied/idle)        │ │
│  └────────┬──────────────────┬─────────────┘ │
│           │                  │               │
│  ┌────────▼────────┐  ┌─────▼─────────────┐ │
│  │  TmuxClient      │  │  JSONLWatcher     │ │
│  │  - new-session    │  │  - tail -f        │ │
│  │  - capture-pane   │  │  - 状態変化検知    │ │
│  │  - send-keys      │  │  - → Notifier     │ │
│  │  - kill-session   │  └─────┬─────────────┘ │
│  └──────────────────┘        │               │
│                       ┌──────▼──────────┐    │
│                       │  Notifier        │    │
│                       │  - OS通知        │    │
│                       │  - バッジ更新     │    │
│                       └─────────────────┘    │
└──────────────────────────────────────────────┘
```

## ディレクトリ構成

```
chatmux/
├── src/
│   ├── main.rs                  # エントリポイント
│   ├── app.rs                   # アプリケーション状態・メインループ
│   ├── tui/
│   │   ├── mod.rs
│   │   ├── sidebar.rs           # セッション一覧コンポーネント
│   │   └── terminal.rs          # ターミナル表示コンポーネント
│   ├── session/
│   │   ├── mod.rs
│   │   ├── manager.rs           # セッションライフサイクル管理
│   │   └── model.rs             # Session データモデル
│   ├── tmux/
│   │   ├── mod.rs
│   │   └── client.rs            # tmux コマンドラッパー
│   ├── watcher/
│   │   ├── mod.rs
│   │   └── jsonl.rs             # JSONL ファイル監視
│   └── notify/
│       ├── mod.rs
│       └── notifier.rs          # OS 通知 + バッジ管理
├── Cargo.toml
└── docs/
```

## データフロー

### セッション起動
```
ユーザー操作 [New Session]
  → SessionManager.Create(projectDir)
    → TmuxClient.NewSession(name, "claude", cwd=projectDir)
    → JSONLWatcher.Watch(~/.claude/projects/-{path}/)
    → SessionList に追加
```

### ターミナル表示（定期リフレッシュ）
```
Tick (100ms 間隔)
  → TmuxClient.CapturePane(session) → ANSI 付きテキスト
  → TerminalView.Update(content)
  → View() でそのまま描画
```

### 通知フロー
```
JSONL に新規行追加
  → JSONLWatcher が検知
  → type: "assistant" メッセージを検出
  → SessionManager.SetStatus(session, "replied")
  → Notifier.Send("proj-A replied")  // OS 通知
  → Sidebar バッジ更新                // TUI 内バッジ
```

### 入力転送
```
ユーザーのキー入力（TerminalView がフォーカス中）
  → TmuxClient.SendKeys(session, key)
  → tmux が claude プロセスに転送
```

## 主な技術課題

### 1. capture-pane の ANSI を Ratatui で描画
- `tmux capture-pane -e -p` は ANSI エスケープシーケンス付きで出力
- Ratatui の View() に渡す文字列に ANSI が含まれていれば、ターミナルが解釈して色付きで表示される
- 課題: ratatui Style のレイアウト計算が ANSI 文字列の幅を誤認する可能性
- 対策: 固定幅の領域に収める、または ratatui Style.Width() の ANSI 対応を利用

### 2. キー入力の転送
- Ratatui がキーイベントを消費 → tmux に転送する仕組みが必要
- TerminalView にフォーカスがあるとき、キーを capture せず send-keys に流す
- 特殊キー（Ctrl-C 等）のハンドリング

### 3. JSONL の状態検知精度
- Claude Code の JSONL フォーマットに依存（非公式）
- assistant メッセージ = "replied"、tool_use = "working" と判定
- フォーマット変更時の追従が必要
