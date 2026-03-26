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

/// Get the modification time of a file.
pub fn file_modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}
