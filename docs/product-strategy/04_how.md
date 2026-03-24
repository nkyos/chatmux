# How — どうやって実現するのか

## 対象エージェント

- **Phase 1**: Claude Code
- **将来**: OpenAI Codex

## Claude Code との接続方法 — 選択肢

### 方式 A: PTY ラッパー

chatmux が `claude` を PTY（仮想端末）の中で起動し、入出力を中継する。

```
chatmux → PTY → claude (interactive TUI)
         ↕
    入出力をキャプチャ
```

- ✅ Claude Code のインタラクティブ TUI がそのまま使える
- ✅ 「セッション開いたらターミナル画面が表示される」を自然に実現
- ✅ 入力も送れる
- ⚠️ 出力はエスケープシーケンス混じりの生テキスト → パースが面倒
- ⚠️ 「新しい返信が来た」の検出にヒューリスティックが必要

### 方式 B: stream-json モード

Claude Code は `claude -p --output-format stream-json --input-format stream-json` で
構造化された JSON ストリームとして会話できる。

```
chatmux → stdin (JSON) → claude -p --output-format stream-json
        ← stdout (JSON) ←
```

- ✅ 入出力が構造化 JSON → パースが容易
- ✅ プログラマティックに制御できる
- ❌ `-p` モード = 非インタラクティブ（TUI なし）
- ❌ ユーザー確認（permission prompt）のフローが変わる
- ❌ 「ターミナル画面を見せる」体験にならない

### 方式 C: JSONL ファイル監視

Claude Code は会話履歴を `~/.claude/projects/-{path}/{sessionId}.jsonl` に書く。
これを watch して会話内容を取得する。

```
claude (通常起動) → JSONL に書き込み
                      ↑
chatmux が inotify/FSEvents で監視
```

- ✅ 既存のターミナルセッションも拾える（非侵入的）
- ✅ 会話内容が構造化されている
- ❌ 読み取り専用（入力を送れない）
- ❌ リアルタイム性は OS のファイル監視に依存

### 方式 D: ハイブリッド（推奨候補）

**chatmux から起動** → PTY ラッパー（方式 A）でターミナル画面をそのまま埋め込む
**既存セッションの検出・通知** → JSONL 監視（方式 C）で「返信来たよ」を通知

```
┌─ chatmux ──────────────────────────────────┐
│                                             │
│  [セッション一覧]  [セッション詳細]          │
│                                             │
│  ● proj-A #1      ┌──────────────────────┐  │
│    (返信あり)      │                      │  │
│  ○ proj-B #2      │  PTY 埋め込み or     │  │
│    (作業中)        │  会話ビュー          │  │
│  ○ proj-C #3      │                      │  │
│    (idle)          │                      │  │
│                    └──────────────────────┘  │
│                                             │
│  🔔 proj-A #1 replied                      │
└─────────────────────────────────────────────┘
```

- chatmux 起動セッション: `chatmux run claude` → PTY でラップ → 画面をそのまま表示
- 既存セッション発見: `~/.claude/sessions/*.json` を監視 → 検出
- 通知: JSONL を tail -f 的に監視 → assistant メッセージが来たら通知

## Claude Code の内部データ（調査結果）

### セッション管理
- `~/.claude/sessions/{pid}.json` — 稼働中セッションの参照
  ```json
  { "pid": 66721, "sessionId": "uuid", "cwd": "/path", "startedAt": timestamp }
  ```

### 会話履歴
- `~/.claude/projects/-{path}/{sessionId}.jsonl` — 全メッセージの JSONL
- レコード種別: `progress`（assistant応答・tool_use）、`snapshot` 等

### CLI オプション（活用可能）
- `--session-id <uuid>` — セッション ID 指定
- `--name <name>` — セッション表示名
- `-p --output-format stream-json` — 構造化出力
- `--input-format stream-json` — 構造化入力
- `claude --resume <session-id>` — セッション再開
- `claude mcp serve` — MCP サーバーモード

## UI 形態 — 未定

| 選択肢 | PTY埋め込み | ネイティブ通知 | 開発コスト |
|--------|:----------:|:------------:|:---------:|
| TUI (ターミナル) | ◎ | △ | 低 |
| Web アプリ (xterm.js等) | ○ | △ | 中 |
| デスクトップアプリ (Tauri等) | ○ | ◎ | 中〜高 |
| エディタ拡張 | △ | ○ | 中 |

→ **要議論**

## 次のステップ

1. UI 形態の決定
2. 最小 PoC: 1つの Claude Code セッションを PTY ラップして表示
3. JSONL 監視による通知の PoC
