use crate::agent::{AgentKind, FileStamp};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant, SystemTime};

/// Return the current time as a Unix epoch (seconds).
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Working,
    Replied,
    Read,
    /// Agent is waiting for user input (permission prompt, question, etc.).
    InputRequired,
}

impl SessionStatus {
    pub fn icon(&self) -> &str {
        match self {
            Self::Working => "⏳",
            Self::Replied => "🔴",
            Self::Read => "✅",
            Self::InputRequired => "💬",
        }
    }

    pub fn sort_priority(&self) -> u8 {
        match self {
            Self::InputRequired => 0,
            Self::Replied => 1,
            Self::Working => 2,
            Self::Read => 3,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Working => "working",
            Self::Replied => "replied",
            Self::Read => "read",
            Self::InputRequired => "input",
        }
    }
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SessionStatus {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "working" => Ok(Self::Working),
            "replied" => Ok(Self::Replied),
            "read" => Ok(Self::Read),
            "input" => Ok(Self::InputRequired),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    StatusPriority,
    LastActivity,
    Manual,
}

impl SortMode {
    pub fn label(&self) -> &str {
        match self {
            Self::StatusPriority => "status",
            Self::LastActivity => "activity",
            Self::Manual => "manual",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::StatusPriority => Self::LastActivity,
            Self::LastActivity => Self::Manual,
            Self::Manual => Self::StatusPriority,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    /// Internal name used for tmux session identification.
    pub name: String,
    /// Working directory for this session.
    pub cwd: String,
    /// Display-friendly project name (last component of cwd).
    pub project_name: String,
    /// Which AI agent this session runs.
    pub agent_kind: AgentKind,
    /// User-defined task label (e.g. "fix auth bug").
    pub task_label: Option<String>,
    /// Last user prompt extracted from the JSONL file.
    pub last_prompt: Option<String>,
    /// Last assistant reply snippet extracted from the JSONL file.
    pub last_reply: Option<String>,
    /// Current status.
    pub status: SessionStatus,
    /// When the last activity was detected (monotonic, for sorting within a run).
    pub last_activity: Instant,
    /// When the last activity was detected (wall-clock epoch, for display and persistence).
    pub last_activity_epoch: u64,
    /// Cached path to the active JSONL file (resolved lazily).
    pub jsonl_path: Option<PathBuf>,
    /// Last known file stamp (mtime + len) of the JSONL file.
    pub jsonl_stamp: Option<FileStamp>,
    /// True when an external tmux client is directly attached to this session.
    /// TUI skips resizing while this is true.
    pub attached_externally: bool,
    /// Git branch name for this session's cwd (if inside a git repo).
    pub branch: Option<String>,
    /// Agent-side session ID (UUID) for resume support.
    pub agent_session_id: Option<String>,
}

impl Session {
    pub fn new(name: String, cwd: String, agent_kind: AgentKind) -> Self {
        let project_name = std::path::Path::new(&cwd)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.clone());
        let branch = detect_git_branch(&cwd);

        let now = Instant::now();
        let epoch = now_epoch();
        Self {
            name,
            cwd,
            project_name,
            agent_kind,
            task_label: None,
            last_prompt: None,
            last_reply: None,
            status: SessionStatus::Read,
            last_activity: now,
            last_activity_epoch: epoch,
            jsonl_path: None,
            jsonl_stamp: None,
            attached_externally: false,
            branch,
            agent_session_id: None,
        }
    }

    pub fn display_label(&self) -> &str {
        self.task_label
            .as_deref()
            .or(self.last_prompt.as_deref())
            .unwrap_or(&self.project_name)
    }

    pub fn elapsed_display(&self) -> String {
        let secs = now_epoch().saturating_sub(self.last_activity_epoch);
        if secs < 60 {
            "now".into()
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else {
            format!("{}h ago", secs / 3600)
        }
    }

    /// Update both the monotonic and wall-clock activity timestamps.
    pub fn touch_activity(&mut self) {
        self.last_activity = Instant::now();
        self.last_activity_epoch = now_epoch();
    }

    /// Set activity timestamps from a saved epoch value (for restore).
    pub fn set_activity_from_epoch(&mut self, epoch: u64) {
        self.last_activity_epoch = epoch;
        let elapsed = now_epoch().saturating_sub(epoch);
        self.last_activity = Instant::now()
            .checked_sub(Duration::from_secs(elapsed))
            .unwrap_or_else(Instant::now);
    }

    /// Refresh the git branch from the session's cwd.
    pub fn refresh_branch(&mut self) {
        self.branch = detect_git_branch(&self.cwd);
    }

    /// Convert to a history entry for recording session end.
    pub fn to_history_entry(&self) -> super::state::HistoryEntry {
        super::state::HistoryEntry {
            cwd: self.cwd.clone(),
            project_name: self.project_name.clone(),
            agent_kind: self.agent_kind,
            task_label: self.task_label.clone(),
            last_prompt: self.last_prompt.clone(),
            ended_at: now_epoch(),
        }
    }
}

/// Detect the current git branch by reading `.git/HEAD`.
/// Returns `None` if the directory is not a git repo.
pub fn detect_git_branch(cwd: &str) -> Option<String> {
    let mut dir = Path::new(cwd);
    loop {
        let dot_git = dir.join(".git");
        let head_path = if dot_git.is_file() {
            // Worktree: .git is a file containing "gitdir: <path>"
            let content = std::fs::read_to_string(&dot_git).ok()?;
            let gitdir = content.trim().strip_prefix("gitdir: ")?;
            Path::new(gitdir).join("HEAD")
        } else {
            dot_git.join("HEAD")
        };
        if let Ok(content) = std::fs::read_to_string(&head_path) {
            let content = content.trim();
            if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
                return Some(branch.to_string());
            }
            // Detached HEAD — return short hash.
            if content.len() >= 8 {
                return Some(content[..8].to_string());
            }
            return None;
        }
        dir = dir.parent()?;
    }
}
