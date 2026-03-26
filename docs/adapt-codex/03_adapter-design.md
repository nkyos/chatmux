# アダプタインターフェース設計

## 概要

エージェントごとの差異を吸収する `Agent` トレイトを導入し、
SessionManager と TmuxClient がエージェント非依存で動作できるようにする。

## Agent トレイト

```rust
/// エージェントの種別を識別する列挙型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

/// エージェントごとの振る舞いを定義するトレイト
pub trait Agent: Send + Sync {
    /// エージェントの種別
    fn kind(&self) -> AgentKind;

    /// tmux セッション内で実行するコマンド
    fn command(&self) -> &str;

    /// コマンドの引数（オプション）
    fn args(&self) -> Vec<String> {
        vec![]
    }

    /// サイドバーに表示するアイコン
    fn icon(&self) -> &str;

    /// JSONL ファイルのパスを解決する
    /// cwd からエージェント固有のセッションファイルを探す
    fn find_session_file(&self, cwd: &str) -> Option<PathBuf>;

    /// JSONL/セッションファイルからステータスを検知する
    fn detect_status(&self, session_file: &Path) -> Option<DetectedStatus>;

    /// プロジェクト一覧を取得する（エージェント固有の履歴から）
    fn discover_projects(&self) -> Vec<ProjectInfo>;
}
```

## 具体的なアダプタ

### ClaudeCodeAgent

```rust
pub struct ClaudeCodeAgent;

impl Agent for ClaudeCodeAgent {
    fn kind(&self) -> AgentKind { AgentKind::ClaudeCode }
    fn command(&self) -> &str { "claude" }
    fn icon(&self) -> &str { "🟣" }  // Anthropic purple

    fn find_session_file(&self, cwd: &str) -> Option<PathBuf> {
        // 既存の find_active_jsonl ロジック
        // ~/.claude/projects/-{encoded_path}/*.jsonl
    }

    fn detect_status(&self, path: &Path) -> Option<DetectedStatus> {
        // 既存の detect_status ロジック
        // type: assistant + stop_reason: end_turn → Replied
    }

    fn discover_projects(&self) -> Vec<ProjectInfo> {
        // 既存の projects.rs ロジック
        // ~/.claude/projects/ から一覧取得
    }
}
```

### CodexAgent

```rust
pub struct CodexAgent;

impl Agent for CodexAgent {
    fn kind(&self) -> AgentKind { AgentKind::Codex }
    fn command(&self) -> &str { "codex" }
    fn icon(&self) -> &str { "🟢" }  // OpenAI green

    fn find_session_file(&self, cwd: &str) -> Option<PathBuf> {
        // Codex のセッションファイル探索
        // 保存場所は要調査
    }

    fn detect_status(&self, path: &Path) -> Option<DetectedStatus> {
        // Codex JSONL パース
        // turn.completed → Replied
        // item.* (実行中) → Working
    }

    fn discover_projects(&self) -> Vec<ProjectInfo> {
        // Codex の過去セッションからプロジェクト一覧
    }
}
```

## 影響を受けるモジュール

### Session モデル

```rust
pub struct Session {
    pub name: String,
    pub cwd: String,
    pub project_name: String,
    pub agent_kind: AgentKind,       // ← 追加
    pub task_label: Option<String>,
    pub status: SessionStatus,
    // ...
}
```

### TmuxClient

```rust
impl TmuxClient {
    // command 引数を受け取るように変更
    pub fn new_session(
        &self,
        session_name: &str,
        cwd: &str,
        command: &str,           // ← "claude" → 動的に
        args: &[String],         // ← 追加
        width: u16,
        height: u16,
    ) -> Result<()> { ... }
}
```

### SessionManager

```rust
impl SessionManager {
    // agent を引数に取る
    pub fn create(
        &mut self,
        cwd: &str,
        agent: &dyn Agent,       // ← 追加
    ) -> Result<usize> { ... }
}
```

### App (状態ポーリング)

```rust
// poll_session_statuses で agent_kind に応じたアダプタを使う
fn poll_session_statuses(&mut self) {
    for session in &mut self.sessions {
        let agent = get_agent(session.agent_kind);  // AgentKind → &dyn Agent
        // agent.find_session_file(), agent.detect_status() を使用
    }
}
```

## AgentRegistry

動的にエージェントを取得するためのレジストリ:

```rust
pub struct AgentRegistry {
    agents: HashMap<AgentKind, Box<dyn Agent>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        let mut registry = Self { agents: HashMap::new() };
        registry.register(Box::new(ClaudeCodeAgent));
        registry.register(Box::new(CodexAgent));
        registry
    }

    pub fn get(&self, kind: AgentKind) -> &dyn Agent { ... }

    /// インストール済みのエージェントのみ返す
    pub fn available(&self) -> Vec<&dyn Agent> {
        self.agents.values()
            .filter(|a| which::which(a.command()).is_ok())
            .map(|a| a.as_ref())
            .collect()
    }
}
```

## 設計判断

### なぜトレイトか（enum match ではなく）

- エージェント追加時に既存コードを変更しなくてよい（Open/Closed）
- 各アダプタのロジックが独立したファイルに分離できる
- テスト時にモックアダプタを差し込める

### なぜ動的ディスパッチか（ジェネリクスではなく）

- AgentKind は実行時に決まる（ユーザーの選択、永続化からの復元）
- `Box<dyn Agent>` で十分（パフォーマンスクリティカルではない）

### fallback: 画面スクレイピング

JSONL が取得できないエージェント用のフォールバック:

```rust
/// capture-pane の出力からヒューリスティックに状態を判定
fn detect_status_from_screen(output: &str) -> SessionStatus {
    // ">" や "?" プロンプトが末尾にある → Replied / Waiting
    // スピナーや進捗表示 → Working
    // 何もない → Idle
}
```

これは Agent トレイトのデフォルト実装として提供できる。
JSONL が見つからない場合にフォールバックするのが安全。
