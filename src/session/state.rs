use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionEntry {
    pub name: String,
    pub cwd: String,
    pub project_name: String,
    pub task_label: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SavedState {
    pub sessions: Vec<SessionEntry>,
    pub next_id: usize,
}

fn state_path() -> Option<PathBuf> {
    dirs::config_dir().map(|p| p.join("chatmux").join("sessions.json"))
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
