mod claude_code;
mod codex;

pub use claude_code::ClaudeCodeAgent;
pub use codex::CodexAgent;

use crate::session::SessionStatus;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
}

impl Default for AgentKind {
    fn default() -> Self {
        Self::ClaudeCode
    }
}

impl AgentKind {
    pub fn icon(&self) -> &str {
        match self {
            Self::ClaudeCode => "CC",
            Self::Codex => "CX",
        }
    }

    pub fn icon_color(&self) -> Color {
        match self {
            Self::ClaudeCode => Color::Rgb(255, 165, 0),
            Self::Codex => Color::Green,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }
}

pub struct DetectedStatus {
    pub status: SessionStatus,
    pub timestamp: Option<String>,
    pub last_prompt: Option<String>,
    /// Last assistant reply text (truncated snippet for notifications).
    pub last_reply: Option<String>,
}

/// Agent adapter trait: defines how to interact with a specific AI coding agent.
pub trait Agent: Send + Sync {
    fn kind(&self) -> AgentKind;
    fn command(&self) -> &str;
    fn args(&self) -> Vec<String> {
        vec![]
    }

    /// List all session files for a given cwd (used for snapshot before session creation).
    fn list_session_files(&self, cwd: &str) -> Vec<PathBuf>;

    /// Find the active session file (e.g. JSONL) for status detection.
    /// `exclude` contains files to skip (pre-existing + already assigned to other sessions).
    fn find_session_file(&self, cwd: &str, exclude: &[PathBuf]) -> Option<PathBuf>;

    /// Detect session status from the session file.
    fn detect_status(&self, session_file: &Path) -> Option<DetectedStatus>;

    /// Discover recent projects from this agent's history.
    fn discover_projects(&self) -> Vec<String>;
}

pub struct AgentRegistry {
    agents: Vec<Box<dyn Agent>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: vec![Box::new(ClaudeCodeAgent), Box::new(CodexAgent)],
        }
    }

    pub fn get(&self, kind: AgentKind) -> &dyn Agent {
        self.agents
            .iter()
            .find(|a| a.kind() == kind)
            .map(|a| a.as_ref())
            .expect("Agent not registered")
    }

    /// Return agents whose CLI command is available on $PATH.
    pub fn available(&self) -> Vec<&dyn Agent> {
        self.agents
            .iter()
            .filter(|a| command_exists(a.command()))
            .map(|a| a.as_ref())
            .collect()
    }

    /// Discover recent projects from all available agents, deduplicated.
    pub fn discover_all_projects(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::new();
        for agent in self.available() {
            for project in agent.discover_projects() {
                if seen.insert(project.clone()) {
                    all.push(project);
                }
            }
        }
        all
    }
}

fn command_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Snapshot of a file's modification time and size.
/// Comparing both catches writes that don't change mtime (same-second flush).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

/// Get the modification time and size of a file.
pub fn file_stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// Read complete JSONL lines from the tail of a file.
/// Incomplete trailing lines (no terminating newline) are discarded to avoid
/// parsing partially-flushed records. Returns empty Vec on error.
pub fn read_complete_jsonl_tail(path: &Path, tail_bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let Ok(meta) = file.metadata() else {
        return Vec::new();
    };
    let file_len = meta.len();
    let start = file_len.saturating_sub(tail_bytes);

    let mut buf = Vec::new();

    if start > 0 {
        if file.seek(SeekFrom::Start(start)).is_err() {
            return Vec::new();
        }
        if file.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        // Discard the first partial line.
        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            buf.drain(..=pos);
        } else {
            // The entire tail is a single line — read the full file instead.
            buf.clear();
            if file.seek(SeekFrom::Start(0)).is_err() {
                return Vec::new();
            }
            if file.read_to_end(&mut buf).is_err() {
                return Vec::new();
            }
        }
    } else {
        if file.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
    }

    let ends_with_newline = buf.last() == Some(&b'\n');
    let mut lines: Vec<String> = buf
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| String::from_utf8(l.to_vec()).ok())
        .collect();

    // Discard incomplete trailing line (writer hasn't flushed the newline yet).
    if !ends_with_newline && !lines.is_empty() {
        lines.pop();
    }

    lines
}
