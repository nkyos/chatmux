use super::{Agent, AgentKind, DetectedStatus, read_complete_jsonl_tail};
use crate::session::SessionStatus;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

pub struct ClaudeCodeAgent;

impl Agent for ClaudeCodeAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    fn command(&self) -> &str {
        "claude"
    }

    fn args(&self) -> Vec<String> {
        vec!["--dangerously-skip-permissions".to_string()]
    }

    fn list_session_files(&self, cwd: &str) -> Vec<PathBuf> {
        let Some(home) = std::env::var("HOME").ok() else {
            return Vec::new();
        };
        let encoded = encode_project_path(cwd);
        let project_dir = PathBuf::from(&home)
            .join(".claude/projects")
            .join(&encoded);

        if !project_dir.is_dir() {
            return Vec::new();
        }

        fs::read_dir(&project_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "jsonl"))
            .map(|e| e.path())
            .collect()
    }

    fn find_session_file(&self, cwd: &str, exclude: &[PathBuf]) -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let encoded = encode_project_path(cwd);
        let project_dir = PathBuf::from(&home)
            .join(".claude/projects")
            .join(&encoded);

        if !project_dir.is_dir() {
            return None;
        }

        let mut best: Option<(PathBuf, std::time::SystemTime)> = None;

        for entry in fs::read_dir(&project_dir).ok()?.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "jsonl") && !exclude.contains(&path) {
                if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                    if best.as_ref().is_none_or(|(_, t)| modified > *t) {
                        best = Some((path, modified));
                    }
                }
            }
        }

        best.map(|(p, _)| p)
    }

    fn detect_status(&self, session_file: &Path) -> Option<DetectedStatus> {
        let lines = read_complete_jsonl_tail(session_file, 256 * 1024);

        let mut last_type: Option<String> = None;
        let mut last_stop_reason: Option<String> = None;
        let mut last_timestamp: Option<String> = None;
        let mut last_prompt: Option<String> = None;
        let mut last_user_text: Option<String> = None;
        let mut last_assistant_text: Option<String> = None;
        let mut parsed: usize = 0;

        for line in &lines {
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

            parsed += 1;

            match entry_type.as_str() {
                "assistant" => {
                    last_type = Some("assistant".into());
                    last_stop_reason = entry.message.as_ref().and_then(|m| m.stop_reason.clone());
                    if entry.timestamp.is_some() {
                        last_timestamp = entry.timestamp;
                    }
                    // Extract assistant text content for notification snippet.
                    if let Some(ref msg) = entry.message {
                        if let Some(ref content) = msg.content {
                            let text = extract_assistant_text(content);
                            if !text.is_empty() {
                                last_assistant_text = Some(text);
                            }
                        }
                    }
                }
                "user" => {
                    last_type = Some("user".into());
                    last_stop_reason = None;
                    if entry.timestamp.is_some() {
                        last_timestamp = entry.timestamp;
                    }
                    // Extract user text content (skip tool_result entries).
                    if let Some(ref msg) = entry.message {
                        if let Some(ref content) = msg.content {
                            let text = extract_user_text(content);
                            if !text.is_empty() {
                                last_user_text = Some(text);
                            }
                        }
                    }
                }
                "progress" => {
                    last_type = Some("progress".into());
                }
                "last-prompt" => {
                    last_prompt = entry.last_prompt;
                }
                _ => {}
            }
        }

        // If no entries could be parsed, return None to keep previous status.
        if parsed == 0 {
            return None;
        }

        // Prefer the explicit last-prompt entry, fall back to last user text.
        let prompt = last_prompt.or(last_user_text);

        let status = match last_type.as_deref() {
            Some("assistant") => match last_stop_reason.as_deref() {
                Some("end_turn") => SessionStatus::Replied,
                _ => SessionStatus::Working, // tool_use or streaming (null)
            },
            Some("user") | Some("progress") => SessionStatus::Working,
            _ => SessionStatus::Working,
        };

        Some(DetectedStatus {
            status,
            timestamp: last_timestamp,
            last_prompt: prompt,
            last_reply: last_assistant_text,
        })
    }

    fn discover_projects(&self) -> Vec<String> {
        crate::projects::discover_projects()
    }

    fn extract_session_id(&self, jsonl_path: &Path) -> Option<String> {
        jsonl_path.file_stem()?.to_str().map(|s| s.to_string())
    }

    fn resume_args(&self, session_id: Option<&str>) -> Vec<String> {
        match session_id {
            Some(id) => vec![
                "--resume".into(),
                id.into(),
                "--dangerously-skip-permissions".into(),
            ],
            None => vec![
                "--continue".into(),
                "--dangerously-skip-permissions".into(),
            ],
        }
    }
}

/// Encode a filesystem path to Claude's project directory name.
/// `/Users/nkyos/lab/tools/chatmux` → `-Users-nkyos-lab-tools-chatmux`
/// Claude Code also replaces underscores with hyphens.
fn encode_project_path(cwd: &str) -> String {
    cwd.replace('/', "-").replace('_', "-")
}

#[derive(Deserialize)]
struct JsonlEntry {
    r#type: Option<String>,
    message: Option<MessagePart>,
    timestamp: Option<String>,
    /// Present in "last-prompt" type entries.
    #[serde(rename = "lastPrompt")]
    last_prompt: Option<String>,
}

#[derive(Deserialize)]
struct MessagePart {
    stop_reason: Option<String>,
    content: Option<serde_json::Value>,
}

/// Extract assistant text from message content, skipping tool_use entries.
fn extract_assistant_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            // Collect all text blocks from the assistant message.
            let mut texts = Vec::new();
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        texts.push(text.to_string());
                    }
                }
            }
            texts.join("\n")
        }
        _ => String::new(),
    }
}

/// Extract user-typed text from message content, skipping tool_result entries.
fn extract_user_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        return text.to_string();
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}
