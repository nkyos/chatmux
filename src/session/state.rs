use crate::agent::AgentKind;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionEntry {
    pub name: String,
    pub cwd: String,
    pub project_name: String,
    #[serde(default)]
    pub agent_kind: AgentKind,
    pub task_label: Option<String>,
    /// Last user prompt extracted from the JSONL file.
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// Resolved session file path (JSONL), saved so restore preserves the mapping.
    #[serde(default)]
    pub session_file: Option<String>,
    /// Wall-clock timestamp of last activity (Unix epoch seconds).
    #[serde(default)]
    pub last_activity_epoch: Option<u64>,
    /// Session status at save time ("working", "replied", "read").
    #[serde(default)]
    pub status: Option<String>,
    /// JSONL file modification time (Unix epoch seconds) at save time.
    /// Used to skip re-reading unchanged files on restore.
    #[serde(default)]
    pub jsonl_modified_epoch: Option<u64>,
    /// Git branch name for this session's cwd.
    #[serde(default)]
    pub branch: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedState {
    pub sessions: Vec<SessionEntry>,
    pub next_id: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub cwd: String,
    pub project_name: String,
    #[serde(default)]
    pub agent_kind: AgentKind,
    pub task_label: Option<String>,
    #[serde(default)]
    pub last_prompt: Option<String>,
    /// Unix timestamp when the session ended.
    pub ended_at: u64,
}

impl HistoryEntry {
    pub fn elapsed_display(&self) -> String {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let elapsed = now.saturating_sub(self.ended_at);
        if elapsed < 60 {
            "just now".into()
        } else if elapsed < 3600 {
            format!("{}m ago", elapsed / 60)
        } else if elapsed < 86400 {
            format!("{}h ago", elapsed / 3600)
        } else {
            format!("{}d ago", elapsed / 86400)
        }
    }
}

fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("chatmux").join("sessions.json"))
}

fn history_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("chatmux").join("history.json"))
}

pub fn save(state: &SavedState) -> Result<()> {
    let path = state_path().ok_or_else(|| anyhow::anyhow!("Cannot determine config directory"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&path, json)?;
    Ok(())
}

pub fn load() -> Option<SavedState> {
    let path = state_path()?;
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn remove() {
    if let Some(path) = state_path() {
        let _ = fs::remove_file(&path);
    }
}

/// Append a history entry (most recent first, max 100 entries).
pub fn append_history(entry: &HistoryEntry) {
    let Some(path) = history_path() else { return };
    let mut entries = load_history();
    entries.insert(0, entry.clone());
    entries.truncate(100);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        &path,
        serde_json::to_string_pretty(&entries).unwrap_or_default(),
    );
}

pub fn load_history() -> Vec<HistoryEntry> {
    let Some(path) = history_path() else {
        return vec![];
    };
    let Ok(data) = fs::read_to_string(&path) else {
        return vec![];
    };
    serde_json::from_str(&data).unwrap_or_default()
}

pub fn save_history(entries: &[HistoryEntry]) {
    let Some(path) = history_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(
        &path,
        serde_json::to_string_pretty(entries).unwrap_or_default(),
    );
}
