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

    /// Args to use when launching with a deterministic session ID.
    /// Agents that support `--session-id` override this.
    fn launch_args(&self, _session_id: Option<&str>) -> Vec<String> {
        self.args()
    }

    /// Return the expected JSONL path for a given cwd + session ID.
    /// Agents that support deterministic session IDs override this.
    fn session_file_for(&self, _cwd: &str, _session_id: &str) -> Option<PathBuf> {
        None
    }

    /// List all session files for a given cwd (used for snapshot and file resolution).
    fn list_session_files(&self, cwd: &str) -> Vec<PathBuf>;

    /// Detect session status from the session file.
    fn detect_status(&self, session_file: &Path) -> Option<DetectedStatus>;

    /// Discover recent projects from this agent's history.
    fn discover_projects(&self) -> Vec<String>;

    /// Extract the agent's session ID from a JSONL file path or contents.
    fn extract_session_id(&self, _jsonl_path: &Path) -> Option<String> {
        None
    }

    /// Command for resuming a session (may differ from `command()`).
    fn resume_command(&self) -> &str {
        self.command()
    }

    /// Args for resuming a session by ID. Falls back to regular args if no ID.
    fn resume_args(&self, _session_id: Option<&str>) -> Vec<String> {
        self.args()
    }

    /// Args for launching the interactive resume/session picker.
    fn resume_picker_args(&self) -> Vec<String> {
        self.args()
    }
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
pub(crate) fn read_complete_jsonl_tail(path: &Path, tail_bytes: u64) -> Vec<String> {
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
    } else if file.read_to_end(&mut buf).is_err() {
        return Vec::new();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn read_tail_complete_lines() {
        let lines = read_complete_jsonl_tail(&fixture_path("end_turn.jsonl"), 1024 * 1024);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("\"user\""));
        assert!(lines[1].contains("\"last-prompt\""));
        assert!(lines[2].contains("\"end_turn\""));
    }

    #[test]
    fn read_tail_incomplete_trailing_line_discarded() {
        let lines = read_complete_jsonl_tail(&fixture_path("incomplete_tail.jsonl"), 1024 * 1024);
        // The last line has no trailing newline, so it should be discarded.
        // We should get 3 complete lines (user, last-prompt, assistant).
        assert_eq!(lines.len(), 3);
        assert!(lines[2].contains("\"end_turn\""));
    }

    #[test]
    fn read_tail_empty_file() {
        let lines = read_complete_jsonl_tail(&fixture_path("empty.jsonl"), 1024 * 1024);
        assert!(lines.is_empty());
    }

    #[test]
    fn read_tail_nonexistent_file() {
        let lines = read_complete_jsonl_tail(Path::new("/nonexistent/file.jsonl"), 1024 * 1024);
        assert!(lines.is_empty());
    }

    #[test]
    fn read_tail_small_tail_bytes() {
        // When tail_bytes is very small, we only get the tail portion.
        // The function should discard the first partial line from the seek point.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.jsonl");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, r#"{{"type":"line1"}}"#).unwrap();
            writeln!(f, r#"{{"type":"line2"}}"#).unwrap();
            writeln!(f, r#"{{"type":"line3"}}"#).unwrap();
        }
        // Read only last ~20 bytes — should get at most "line3".
        let lines = read_complete_jsonl_tail(&path, 20);
        assert!(!lines.is_empty());
        assert!(lines.last().unwrap().contains("line3"));
    }

    #[test]
    fn file_stamp_returns_some_for_existing_file() {
        let stamp = file_stamp(&fixture_path("end_turn.jsonl"));
        assert!(stamp.is_some());
        let s = stamp.unwrap();
        assert!(s.len > 0);
        assert!(s.modified.is_some());
    }

    #[test]
    fn file_stamp_returns_none_for_missing_file() {
        let stamp = file_stamp(Path::new("/nonexistent/file.jsonl"));
        assert!(stamp.is_none());
    }
}
