use std::path::{Path, PathBuf};

fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("chatmux")
}

pub fn hooks_dir() -> PathBuf {
    state_dir().join("hooks")
}

pub fn events_dir() -> PathBuf {
    state_dir().join("events")
}

const HOOK_SCRIPT: &str = r#"#!/bin/bash
# chatmux hook relay — appends Claude Code hook events to a per-session event file.
# CHATMUX_SESSION is set by chatmux via `tmux new-session -e`.
# If not set, this hook is a no-op (allows claude to run outside chatmux).
[ -z "$CHATMUX_SESSION" ] && exit 0

EVENTS_DIR="${CHATMUX_EVENTS_DIR:-$HOME/.local/state/chatmux/events}"
mkdir -p "$EVENTS_DIR"
cat >> "$EVENTS_DIR/${CHATMUX_SESSION}.jsonl"
"#;

/// Write the hook relay script to disk (idempotent).
pub fn ensure_hook_script() -> std::io::Result<PathBuf> {
    let dir = hooks_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("claude-hook.sh");

    let needs_write = std::fs::read_to_string(&path)
        .map(|existing| existing != HOOK_SCRIPT)
        .unwrap_or(true);

    if needs_write {
        std::fs::write(&path, HOOK_SCRIPT)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }

    Ok(path)
}

/// Ensure the events directory exists.
pub fn ensure_events_dir() -> std::io::Result<PathBuf> {
    let dir = events_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Build the --settings JSON string that injects hooks for all lifecycle events.
pub fn build_hooks_settings_json(hook_script: &Path) -> String {
    let cmd = hook_script.to_string_lossy();
    let events = ["SessionStart", "UserPromptSubmit", "Stop", "Notification"];
    let hooks_entries: Vec<String> = events
        .iter()
        .map(|event| {
            format!(
                r#""{event}":[{{"hooks":[{{"type":"command","command":"bash {cmd}"}}]}}]"#
            )
        })
        .collect();
    format!(r#"{{"hooks":{{{}}}}}"#, hooks_entries.join(","))
}

/// A parsed hook event from Claude Code.
#[derive(Debug)]
pub struct HookEvent {
    pub hook_event_name: String,
    pub session_id: Option<String>,
    #[allow(dead_code)]
    pub transcript_path: Option<String>,
    pub prompt: Option<String>,
    pub last_assistant_message: Option<String>,
    #[allow(dead_code)]
    pub source: Option<String>,
}

/// Read and drain all pending events for a chatmux session.
/// Returns events in order, then truncates the file.
pub fn drain_events(session_name: &str) -> Vec<HookEvent> {
    let path = events_dir().join(format!("{session_name}.jsonl"));
    let content = match std::fs::read_to_string(&path) {
        Ok(c) if !c.is_empty() => c,
        _ => return Vec::new(),
    };

    // Truncate after reading.
    let _ = std::fs::write(&path, "");

    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let val: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(HookEvent {
                hook_event_name: val
                    .get("hook_event_name")?
                    .as_str()?
                    .to_string(),
                session_id: val
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                transcript_path: val
                    .get("transcript_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                prompt: val
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                last_assistant_message: extract_last_assistant_text(&val),
                source: val
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            })
        })
        .collect()
}

fn extract_last_assistant_text(val: &serde_json::Value) -> Option<String> {
    let msg = val.get("last_assistant_message")?;
    // last_assistant_message can be a string or a structured object with content array
    if let Some(s) = msg.as_str() {
        return Some(s.to_string());
    }
    if let Some(content) = msg.get("content").and_then(|c| c.as_array()) {
        let texts: Vec<&str> = content
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect();
        if !texts.is_empty() {
            return Some(texts.join("\n"));
        }
    }
    None
}

/// Clean up event files for sessions that no longer exist.
pub fn cleanup_events(live_sessions: &[String]) {
    let dir = match events_dir().read_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    let live: std::collections::HashSet<&str> =
        live_sessions.iter().map(|s| s.as_str()).collect();

    for entry in dir.flatten() {
        if let Some(name) = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            && !live.contains(name.as_str())
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
