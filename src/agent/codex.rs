use super::{Agent, AgentKind, DetectedStatus, read_complete_jsonl_tail};
use crate::session::SessionStatus;
use serde::Deserialize;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct CodexAgent;

impl Agent for CodexAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn command(&self) -> &str {
        "codex"
    }

    fn args(&self) -> Vec<String> {
        vec!["--yolo".to_string()]
    }

    fn list_session_files(&self, cwd: &str) -> Vec<PathBuf> {
        let Some(home) = std::env::var("HOME").ok() else {
            return Vec::new();
        };
        let sessions_dir = PathBuf::from(&home).join(".codex/sessions");
        if !sessions_dir.is_dir() {
            return Vec::new();
        }
        list_all_sessions_for_cwd(&sessions_dir, cwd)
    }

    fn find_session_file(&self, cwd: &str, exclude: &[PathBuf]) -> Option<PathBuf> {
        // Codex stores sessions at ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
        // The first line of each file contains session_meta with cwd.
        // We find the most recently modified JSONL whose cwd matches.
        let home = std::env::var("HOME").ok()?;
        let sessions_dir = PathBuf::from(&home).join(".codex/sessions");

        if !sessions_dir.is_dir() {
            return None;
        }

        find_latest_session_for_cwd(&sessions_dir, cwd, exclude)
    }

    fn detect_status(&self, session_file: &Path) -> Option<DetectedStatus> {
        detect_codex_status(session_file)
    }

    fn discover_projects(&self) -> Vec<String> {
        discover_codex_projects()
    }

    fn extract_session_id(&self, jsonl_path: &Path) -> Option<String> {
        let file = fs::File::open(jsonl_path).ok()?;
        let mut reader = BufReader::new(file);
        let mut first_line = String::new();
        reader.read_line(&mut first_line).ok()?;
        let entry: CodexEntry = serde_json::from_str(&first_line).ok()?;
        if entry.r#type.as_deref() == Some("session_meta") {
            entry.payload?.id
        } else {
            None
        }
    }

    fn resume_command(&self) -> &str {
        "codex"
    }

    fn resume_args(&self, session_id: Option<&str>) -> Vec<String> {
        match session_id {
            Some(id) => vec![
                "resume".into(),
                id.into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
            ],
            None => vec![
                "resume".into(),
                "--last".into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
            ],
        }
    }
}

/// List all Codex session JSONL files matching the given cwd.
fn list_all_sessions_for_cwd(sessions_dir: &Path, cwd: &str) -> Vec<PathBuf> {
    let mut results = Vec::new();
    let mut year_dirs = list_sorted_dirs(sessions_dir).unwrap_or_default();
    year_dirs.reverse();

    for year_dir in year_dirs.iter().take(2) {
        let mut month_dirs = list_sorted_dirs(year_dir).unwrap_or_default();
        month_dirs.reverse();

        for month_dir in month_dirs.iter().take(3) {
            let mut day_dirs = list_sorted_dirs(month_dir).unwrap_or_default();
            day_dirs.reverse();

            for day_dir in day_dirs.iter().take(31) {
                if let Ok(entries) = fs::read_dir(day_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().is_some_and(|e| e == "jsonl")
                            && session_matches_cwd(&path, cwd)
                        {
                            results.push(path);
                        }
                    }
                }
            }
        }
    }
    results
}

/// Find the most recently modified Codex session JSONL file whose cwd matches.
/// Scans recent date directories (latest first) to avoid scanning thousands of files.
fn find_latest_session_for_cwd(
    sessions_dir: &Path,
    cwd: &str,
    exclude: &[PathBuf],
) -> Option<PathBuf> {
    // Collect year dirs, sorted descending.
    let mut year_dirs = list_sorted_dirs(sessions_dir)?;
    year_dirs.reverse();

    for year_dir in year_dirs.iter().take(2) {
        let mut month_dirs = list_sorted_dirs(year_dir)?;
        month_dirs.reverse();

        for month_dir in month_dirs.iter().take(3) {
            let mut day_dirs = list_sorted_dirs(month_dir)?;
            day_dirs.reverse();

            for day_dir in day_dirs.iter().take(31) {
                if let Some(path) = find_session_in_dir(day_dir, cwd, exclude) {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Find the most recently modified JSONL in a day directory matching the given cwd.
fn find_session_in_dir(day_dir: &Path, cwd: &str, exclude: &[PathBuf]) -> Option<PathBuf> {
    let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();

    for entry in fs::read_dir(day_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                candidates.push((path, modified));
            }
        }
    }

    // Sort by most recently modified first.
    candidates.sort_by(|a, b| b.1.cmp(&a.1));

    for (path, _) in candidates {
        if !exclude.contains(&path) && session_matches_cwd(&path, cwd) {
            return Some(path);
        }
    }

    None
}

/// Check if a Codex session JSONL's cwd matches the expected cwd.
fn session_matches_cwd(path: &Path, expected_cwd: &str) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).is_err() {
        return false;
    }

    let Ok(entry) = serde_json::from_str::<CodexEntry>(&first_line) else {
        return false;
    };

    if entry.r#type.as_deref() == Some("session_meta") {
        if let Some(ref payload) = entry.payload {
            return payload.cwd.as_deref() == Some(expected_cwd);
        }
    }

    false
}

fn list_sorted_dirs(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    Some(dirs)
}

/// Detect Codex session status from the tail of a JSONL file.
fn detect_codex_status(session_file: &Path) -> Option<DetectedStatus> {
    let lines = read_complete_jsonl_tail(session_file, 256 * 1024);

    let mut last_event_type: Option<String> = None;
    let mut last_payload_type: Option<String> = None;
    let mut last_role: Option<String> = None;
    let mut last_timestamp: Option<String> = None;
    let mut last_user_message: Option<String> = None;
    let mut last_assistant_text: Option<String> = None;
    let mut parsed: usize = 0;

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Ok(entry) = serde_json::from_str::<CodexEntry>(trimmed) else {
            continue;
        };

        if let Some(ref t) = entry.r#type {
            parsed += 1;
            last_event_type = Some(t.clone());
            if entry.timestamp.is_some() {
                last_timestamp = entry.timestamp.clone();
            }
            if let Some(ref payload) = entry.payload {
                last_payload_type = payload.r#type.clone();
                last_role = payload.role.clone();
                // Track user messages.
                if payload.r#type.as_deref() == Some("user_message") {
                    if let Some(ref text) = payload.text {
                        last_user_message = Some(text.clone());
                    }
                }
                if payload.role.as_deref() == Some("user") {
                    if let Some(ref text) = payload.text {
                        last_user_message = Some(text.clone());
                    }
                }
                // Track assistant reply text.
                if payload.r#type.as_deref() == Some("agent_message")
                    || payload.role.as_deref() == Some("assistant")
                {
                    if let Some(ref text) = payload.text {
                        last_assistant_text = Some(text.clone());
                    }
                }
            }
        }
    }

    // If no entries could be parsed, return None to keep previous status.
    if parsed == 0 {
        return None;
    }

    let status = match (
        last_event_type.as_deref(),
        last_payload_type.as_deref(),
        last_role.as_deref(),
    ) {
        // task_complete = agent finished responding
        (Some("event_msg"), Some("task_complete"), _) => SessionStatus::Replied,
        // agent sent a message = replied (often precedes task_complete)
        (Some("event_msg"), Some("agent_message"), _) => SessionStatus::Replied,
        // assistant message = replied
        (Some("response_item"), Some("message"), Some("assistant")) => SessionStatus::Replied,
        // token_count follows task_complete, treat as replied
        (Some("event_msg"), Some("token_count"), _) => SessionStatus::Replied,
        // task_started = working
        (Some("event_msg"), Some("task_started"), _) => SessionStatus::Working,
        // user sent a message = working (agent will process it)
        (Some("event_msg"), Some("user_message"), _) => SessionStatus::Working,
        (Some("response_item"), Some("message"), Some("user")) => SessionStatus::Working,
        // function call / execution = working
        (Some("response_item"), Some("function_call"), _) => SessionStatus::Working,
        (Some("response_item"), Some("function_call_output"), _) => SessionStatus::Working,
        // reasoning = working
        (Some("response_item"), Some("reasoning"), _) => SessionStatus::Working,
        // turn_context = between turns, idle-ish
        (Some("turn_context"), _, _) => SessionStatus::Working,
        // developer/system messages at start = idle
        (Some("response_item"), Some("message"), Some("developer")) => SessionStatus::Working,
        // default
        _ => SessionStatus::Working,
    };

    Some(DetectedStatus {
        status,
        timestamp: last_timestamp,
        last_prompt: last_user_message,
        last_reply: last_assistant_text,
    })
}

/// Discover projects from Codex's session history.
/// Scans recent session files for unique cwd values.
fn discover_codex_projects() -> Vec<String> {
    let Some(home) = std::env::var("HOME").ok() else {
        return Vec::new();
    };
    let sessions_dir = PathBuf::from(&home).join(".codex/sessions");
    if !sessions_dir.is_dir() {
        return Vec::new();
    }

    let mut seen = std::collections::HashSet::new();
    let mut projects: Vec<(String, std::time::SystemTime)> = Vec::new();

    // Scan recent date directories.
    let mut year_dirs = list_sorted_dirs(&sessions_dir).unwrap_or_default();
    year_dirs.reverse();

    'outer: for year_dir in year_dirs.iter().take(2) {
        let mut month_dirs = list_sorted_dirs(year_dir).unwrap_or_default();
        month_dirs.reverse();

        for month_dir in month_dirs.iter().take(6) {
            let mut day_dirs = list_sorted_dirs(month_dir).unwrap_or_default();
            day_dirs.reverse();

            for day_dir in day_dirs.iter().take(31) {
                let Ok(entries) = fs::read_dir(day_dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.extension().is_some_and(|e| e == "jsonl") {
                        continue;
                    }
                    if let Some(cwd) = extract_cwd(&path) {
                        if seen.insert(cwd.clone()) {
                            let modified = entry
                                .metadata()
                                .ok()
                                .and_then(|m| m.modified().ok())
                                .unwrap_or(std::time::UNIX_EPOCH);
                            projects.push((cwd, modified));
                            if projects.len() >= 50 {
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }

    // Sort by most recently modified.
    projects.sort_by(|a, b| b.1.cmp(&a.1));
    projects
        .into_iter()
        .filter(|(p, _)| Path::new(p).is_dir())
        .map(|(p, _)| p)
        .collect()
}

/// Extract cwd from the first line of a Codex session JSONL.
fn extract_cwd(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut first_line = String::new();
    reader.read_line(&mut first_line).ok()?;

    let entry: CodexEntry = serde_json::from_str(&first_line).ok()?;
    if entry.r#type.as_deref() == Some("session_meta") {
        entry.payload?.cwd
    } else {
        None
    }
}

/// Minimal JSON structure for reading Codex JSONL entries.
#[derive(Deserialize)]
struct CodexEntry {
    r#type: Option<String>,
    timestamp: Option<String>,
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    r#type: Option<String>,
    role: Option<String>,
    cwd: Option<String>,
    text: Option<String>,
    id: Option<String>,
}
