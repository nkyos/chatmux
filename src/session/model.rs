use std::path::PathBuf;
use std::time::{Instant, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Working,
    Replied,
    Waiting,
    Idle,
}

impl SessionStatus {
    pub fn icon(&self) -> &str {
        match self {
            Self::Working => "⏳",
            Self::Replied => "🔴",
            Self::Waiting => "⚠️",
            Self::Idle => "💤",
        }
    }

    pub fn sort_priority(&self) -> u8 {
        match self {
            Self::Waiting => 0,
            Self::Replied => 1,
            Self::Working => 2,
            Self::Idle => 3,
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
    /// User-defined task label (e.g. "fix auth bug").
    pub task_label: Option<String>,
    /// Current status.
    pub status: SessionStatus,
    /// When this session was created.
    pub created_at: Instant,
    /// When the last activity was detected.
    pub last_activity: Instant,
    /// Cached path to the active JSONL file (resolved lazily).
    pub jsonl_path: Option<PathBuf>,
    /// Last known modification time of the JSONL file.
    pub jsonl_modified: Option<SystemTime>,
}

impl Session {
    pub fn new(name: String, cwd: String) -> Self {
        let project_name = std::path::Path::new(&cwd)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| cwd.clone());

        let now = Instant::now();
        Self {
            name,
            cwd,
            project_name,
            task_label: None,
            status: SessionStatus::Working,
            created_at: now,
            last_activity: now,
            jsonl_path: None,
            jsonl_modified: None,
        }
    }

    pub fn display_label(&self) -> &str {
        self.task_label
            .as_deref()
            .unwrap_or(&self.project_name)
    }

    pub fn elapsed_display(&self) -> String {
        let secs = self.last_activity.elapsed().as_secs();
        if secs < 60 {
            "now".into()
        } else if secs < 3600 {
            format!("{}m ago", secs / 60)
        } else {
            format!("{}h ago", secs / 3600)
        }
    }
}
