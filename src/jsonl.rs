use crate::session::SessionStatus;
use serde::Deserialize;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Encode a filesystem path to Claude's project directory name.
/// `/Users/nkyos/lab/tools/chatmux` → `-Users-nkyos-lab-tools-chatmux`
fn encode_project_path(cwd: &str) -> String {
    cwd.replace('/', "-")
}

/// Find the most recently modified JSONL file for a given project directory.
pub fn find_active_jsonl(cwd: &str) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let encoded = encode_project_path(cwd);
    let project_dir = PathBuf::from(&home)
        .join(".claude/projects")
        .join(&encoded);

    if !project_dir.is_dir() {
        return None;
    }

    let mut best: Option<(PathBuf, SystemTime)> = None;

    for entry in fs::read_dir(&project_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                if best.as_ref().is_none_or(|(_, t)| modified > *t) {
                    best = Some((path, modified));
                }
            }
        }
    }

    best.map(|(p, _)| p)
}

/// Get the file modification time.
pub fn file_modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

/// Minimal JSON structure for reading JSONL entries.
/// We only parse the fields we need to avoid overhead.
#[derive(Deserialize)]
struct JsonlEntry {
    r#type: Option<String>,
    message: Option<MessagePart>,
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct MessagePart {
    stop_reason: Option<String>,
}

/// Result of status detection.
pub struct DetectedStatus {
    pub status: SessionStatus,
    pub timestamp: Option<String>,
}

/// Read the tail of a JSONL file and determine the session status.
pub fn detect_status(jsonl_path: &Path) -> Option<DetectedStatus> {
    let file = fs::File::open(jsonl_path).ok()?;
    let file_len = file.metadata().ok()?.len();

    let mut reader = BufReader::new(file);

    // Read the last ~16KB to find recent entries.
    let seek_pos = file_len.saturating_sub(16384);
    if seek_pos > 0 {
        reader.seek(SeekFrom::Start(seek_pos)).ok()?;
        // Skip the partial first line.
        let mut discard = String::new();
        reader.read_line(&mut discard).ok()?;
    }

    let mut last_type: Option<String> = None;
    let mut last_stop_reason: Option<String> = None;
    let mut last_timestamp: Option<String> = None;

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(entry) = serde_json::from_str::<JsonlEntry>(trimmed) else {
            continue;
        };

        let Some(ref entry_type) = entry.r#type else {
            continue;
        };

        match entry_type.as_str() {
            "assistant" => {
                last_type = Some("assistant".into());
                last_stop_reason = entry.message.and_then(|m| m.stop_reason);
                if entry.timestamp.is_some() {
                    last_timestamp = entry.timestamp;
                }
            }
            "user" => {
                last_type = Some("user".into());
                last_stop_reason = None;
                if entry.timestamp.is_some() {
                    last_timestamp = entry.timestamp;
                }
            }
            "progress" => {
                last_type = Some("progress".into());
            }
            _ => {}
        }
    }

    let status = match last_type.as_deref() {
        Some("assistant") => match last_stop_reason.as_deref() {
            Some("end_turn") => SessionStatus::Replied,
            _ => SessionStatus::Working, // tool_use or streaming (null)
        },
        Some("user") | Some("progress") => SessionStatus::Working,
        _ => SessionStatus::Idle,
    };

    Some(DetectedStatus {
        status,
        timestamp: last_timestamp,
    })
}
