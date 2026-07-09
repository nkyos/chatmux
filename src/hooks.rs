use std::path::{Path, PathBuf};

pub(crate) fn state_dir() -> PathBuf {
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
# chatmux hook relay — writes each Claude Code hook event to its own file.
# CHATMUX_SESSION is set by chatmux via `tmux new-session -e`.
# If not set, this hook is a no-op (allows claude to run outside chatmux).
[ -z "$CHATMUX_SESSION" ] && exit 0

EVENTS_DIR="${CHATMUX_EVENTS_DIR:-$HOME/.local/state/chatmux/events}"
mkdir -p "$EVENTS_DIR"

# Write to a hidden temp file first, then rename: the .evt file appears
# atomically, so the reader never sees a partially written event.
evt="${CHATMUX_SESSION}.$$.${RANDOM}${RANDOM}.evt"
cat > "$EVENTS_DIR/.$evt.tmp" && mv "$EVENTS_DIR/.$evt.tmp" "$EVENTS_DIR/$evt"
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
    let cmd = format!("bash \"{}\"", hook_script.to_string_lossy());
    let hook_entry = serde_json::json!([{
        "hooks": [{
            "type": "command",
            "command": cmd,
        }]
    }]);
    let settings = serde_json::json!({
        "hooks": {
            "SessionStart": hook_entry,
            "UserPromptSubmit": hook_entry,
            "Stop": hook_entry,
            "Notification": hook_entry,
        }
    });
    settings.to_string()
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

/// Build a HookEvent from a parsed JSON value.
fn event_from_value(val: &serde_json::Value) -> Option<HookEvent> {
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
        last_assistant_message: extract_last_assistant_text(val),
        source: val
            .get("source")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// Parse JSONL content into HookEvent entries.
fn parse_events(content: &str) -> Vec<HookEvent> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let val: serde_json::Value = serde_json::from_str(line).ok()?;
            event_from_value(&val)
        })
        .collect()
}

/// Parse the content of a single .evt file (one JSON event, possibly
/// pretty-printed). Falls back to line-based parsing.
fn parse_event_file(content: &str) -> Vec<HookEvent> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(content) {
        return event_from_value(&val).into_iter().collect();
    }
    parse_events(content)
}

/// Read and drain all pending events for a chatmux session.
///
/// The hook script writes one file per event (`{name}.{pid}.{rand}.evt`)
/// via atomic rename, so a file is only ever seen fully written — this
/// avoids the torn reads possible with a shared append log. Files are
/// processed in mtime order. Legacy append logs (`{name}.jsonl`) from
/// older hook scripts are drained first.
pub fn drain_events(session_name: &str) -> Vec<HookEvent> {
    drain_events_in(&events_dir(), session_name)
}

fn drain_events_in(dir: &Path, session_name: &str) -> Vec<HookEvent> {
    let mut events = Vec::new();

    // Legacy append-log files written by older hook scripts.
    let processing = dir.join(format!("{session_name}.jsonl.processing"));
    let legacy = dir.join(format!("{session_name}.jsonl"));
    for path in [&processing, &legacy] {
        if let Ok(content) = std::fs::read_to_string(path) {
            events.extend(parse_events(&content));
            let _ = std::fs::remove_file(path);
        }
    }

    // Per-event files, ordered by mtime.
    let Ok(entries) = dir.read_dir() else {
        return events;
    };
    let prefix = format!("{session_name}.");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let fname = path.file_name()?.to_str()?;
            if !fname.starts_with(&prefix) || !fname.ends_with(".evt") {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, path))
        })
        .collect();
    files.sort_by_key(|(t, _)| *t);

    for (_, path) in files {
        if let Ok(content) = std::fs::read_to_string(&path) {
            events.extend(parse_event_file(&content));
        }
        let _ = std::fs::remove_file(&path);
    }

    events
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

/// Extract the session name from an event filename.
/// Handles per-event files (`{name}.{pid}.{rand}.evt`), their hidden temp
/// files (`.{name}.{pid}.{rand}.evt.tmp`), and legacy append logs
/// (`{name}.jsonl`, `{name}.jsonl.processing`). Session names contain no dots.
fn event_file_session_name(path: &std::path::Path) -> Option<String> {
    let fname = path.file_name()?.to_str()?;
    if let Some(rest) = fname
        .strip_suffix(".jsonl.processing")
        .or_else(|| fname.strip_suffix(".jsonl"))
    {
        return Some(rest.to_string());
    }
    if let Some(rest) = fname.strip_suffix(".evt") {
        return rest.split('.').next().map(|s| s.to_string());
    }
    if let Some(rest) = fname
        .strip_prefix('.')
        .and_then(|r| r.strip_suffix(".evt.tmp"))
    {
        return rest.split('.').next().map(|s| s.to_string());
    }
    None
}

/// Clean up event files for sessions that no longer exist.
/// Handles both `.jsonl` and `.jsonl.processing` remnants.
pub fn cleanup_events(live_sessions: &[String]) {
    let dir = match events_dir().read_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    let live: std::collections::HashSet<&str> =
        live_sessions.iter().map(|s| s.as_str()).collect();

    for entry in dir.flatten() {
        if let Some(name) = event_file_session_name(&entry.path())
            && !live.contains(name.as_str())
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_hooks_settings_json_valid() {
        let path = Path::new("/tmp/hooks/claude-hook.sh");
        let json = build_hooks_settings_json(path);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        let hooks = val.get("hooks").unwrap();
        assert!(hooks.get("SessionStart").is_some());
        assert!(hooks.get("Stop").is_some());
        let cmd = hooks["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, "bash \"/tmp/hooks/claude-hook.sh\"");
    }

    #[test]
    fn build_hooks_settings_json_path_with_spaces() {
        let path = Path::new("/tmp/my hooks/claude hook.sh");
        let json = build_hooks_settings_json(path);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cmd = val["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, "bash \"/tmp/my hooks/claude hook.sh\"");
    }

    #[test]
    fn parse_events_basic() {
        let line = r#"{"hook_event_name":"Stop","session_id":"abc"}"#;
        let events = parse_events(line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].hook_event_name, "Stop");
        assert_eq!(events[0].session_id.as_deref(), Some("abc"));
    }

    #[test]
    fn event_file_session_name_jsonl() {
        let p = Path::new("/tmp/events/s0.jsonl");
        assert_eq!(event_file_session_name(p), Some("s0".into()));
    }

    #[test]
    fn event_file_session_name_processing() {
        let p = Path::new("/tmp/events/s0.jsonl.processing");
        assert_eq!(event_file_session_name(p), Some("s0".into()));
    }

    #[test]
    fn event_file_session_name_evt() {
        let p = Path::new("/tmp/events/s0.12345.6789.evt");
        assert_eq!(event_file_session_name(p), Some("s0".into()));
    }

    #[test]
    fn event_file_session_name_evt_tmp() {
        let p = Path::new("/tmp/events/.xab12cd34.12345.6789.evt.tmp");
        assert_eq!(event_file_session_name(p), Some("xab12cd34".into()));
    }

    #[test]
    fn parse_event_file_single_object() {
        let content = r#"{"hook_event_name":"Stop","session_id":"abc"}"#;
        let events = parse_event_file(content);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].hook_event_name, "Stop");
    }

    #[test]
    fn drain_events_reads_evt_files_in_mtime_order() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("s0.100.1.evt");
        let second = dir.path().join("s0.100.2.evt");
        std::fs::write(&first, r#"{"hook_event_name":"UserPromptSubmit"}"#).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&second, r#"{"hook_event_name":"Stop"}"#).unwrap();

        let events = drain_events_in(dir.path(), "s0");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].hook_event_name, "UserPromptSubmit");
        assert_eq!(events[1].hook_event_name, "Stop");
        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[test]
    fn drain_events_ignores_other_sessions_and_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let other = dir.path().join("s10.100.1.evt");
        let tmp = dir.path().join(".s0.100.1.evt.tmp");
        std::fs::write(&other, r#"{"hook_event_name":"Stop"}"#).unwrap();
        std::fs::write(&tmp, r#"{"hook_event_name":"Stop"}"#).unwrap();

        let events = drain_events_in(dir.path(), "s0");
        assert!(events.is_empty());
        assert!(other.exists());
        assert!(tmp.exists());
    }

    #[test]
    fn drain_events_reads_legacy_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("s0.jsonl");
        std::fs::write(&legacy, "{\"hook_event_name\":\"Stop\"}\n").unwrap();

        let events = drain_events_in(dir.path(), "s0");
        assert_eq!(events.len(), 1);
        assert!(!legacy.exists());
    }
}
