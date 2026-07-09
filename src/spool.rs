use crate::agent::AgentKind;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct SpoolEntry {
    pub cwd: String,
    pub project_name: String,
    pub agent_kind: AgentKind,
    pub agent_session_id: Option<String>,
    pub session_file: Option<String>,
    pub task_label: Option<String>,
    pub created_epoch: u64,
    pub branch: Option<String>,
}

pub fn pending_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("chatmux")
        .join("pending")
}

pub fn write_spool(name: &str, entry: &SpoolEntry) -> std::io::Result<()> {
    let dir = pending_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.json"));
    let tmp = dir.join(format!("{name}.json.tmp"));
    let json = serde_json::to_string_pretty(entry)
        .map_err(std::io::Error::other)?;
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn remove_spool(name: &str) {
    let path = pending_dir().join(format!("{name}.json"));
    let _ = std::fs::remove_file(&path);
}

pub fn list_pending() -> Vec<(String, SpoolEntry)> {
    let dir = pending_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_stem()?.to_str()?.to_string();
            if path.extension().is_none_or(|ext| ext != "json") {
                return None;
            }
            let data = std::fs::read_to_string(&path).ok()?;
            let entry: SpoolEntry = serde_json::from_str(&data).ok()?;
            Some((name, entry))
        })
        .collect()
}

pub fn cleanup_stale(live_tmux_sessions: &std::collections::HashSet<String>, max_age_secs: u64) {
    let now = crate::session::model::now_epoch();
    for (name, entry) in list_pending() {
        let full_name = format!("chatmux-{name}");
        if !live_tmux_sessions.contains(&name)
            && !live_tmux_sessions.contains(&full_name)
            && now.saturating_sub(entry.created_epoch) > max_age_secs
        {
            remove_spool(&name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spool_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        // Override pending dir by writing directly
        let entry = SpoolEntry {
            cwd: "/tmp/test".into(),
            project_name: "test".into(),
            agent_kind: AgentKind::ClaudeCode,
            agent_session_id: Some("uuid-123".into()),
            session_file: Some("/path/to/file.jsonl".into()),
            task_label: Some("fix bug".into()),
            created_epoch: 1000,
            branch: Some("main".into()),
        };
        let path = dir.path().join("xabc12345.json");
        let json = serde_json::to_string_pretty(&entry).unwrap();
        std::fs::write(&path, &json).unwrap();

        let data = std::fs::read_to_string(&path).unwrap();
        let loaded: SpoolEntry = serde_json::from_str(&data).unwrap();
        assert_eq!(loaded.cwd, "/tmp/test");
        assert_eq!(loaded.agent_session_id.as_deref(), Some("uuid-123"));
        assert_eq!(loaded.task_label.as_deref(), Some("fix bug"));
    }
}
