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

    fn launch_args(&self, session_id: Option<&str>) -> Vec<String> {
        match session_id {
            Some(id) => vec![
                "--session-id".into(),
                id.into(),
                "--dangerously-skip-permissions".into(),
            ],
            None => self.args(),
        }
    }

    fn session_file_for(&self, cwd: &str, session_id: &str) -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let encoded = encode_project_path(cwd);
        Some(
            PathBuf::from(&home)
                .join(".claude/projects")
                .join(&encoded)
                .join(format!("{session_id}.jsonl")),
        )
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

    fn detect_status(&self, session_file: &Path) -> Option<DetectedStatus> {
        let lines = read_complete_jsonl_tail(session_file, 1024 * 1024);

        let mut last_type: Option<String> = None;
        let mut last_stop_reason: Option<String> = None;
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
                    // Extract assistant text content for notification snippet.
                    if let Some(ref msg) = entry.message
                        && let Some(ref content) = msg.content {
                            let text = extract_assistant_text(content);
                            if !text.is_empty() {
                                last_assistant_text = Some(text);
                            }
                        }
                }
                "user" => {
                    last_type = Some("user".into());
                    last_stop_reason = None;
                    // Extract user text content (skip tool_result entries).
                    if let Some(ref msg) = entry.message
                        && let Some(ref content) = msg.content {
                            let text = extract_user_text(content);
                            if !text.is_empty() {
                                last_user_text = Some(text);
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

        // /clear and /new reset the session without producing an assistant
        // response, so they should not leave status as Working.
        let is_noop_command = last_user_text.as_ref().is_some_and(|t| {
            t.contains("<command-name>/clear</command-name>")
                || t.contains("<command-name>/new</command-name>")
        });

        // Prefer the explicit last-prompt entry, fall back to last user text.
        let prompt = last_prompt.or(last_user_text);

        let status = match last_type.as_deref() {
            Some("assistant") => match last_stop_reason.as_deref() {
                Some("end_turn") => SessionStatus::Replied,
                _ => SessionStatus::Working, // tool_use or streaming (null)
            },
            Some("user") if is_noop_command => SessionStatus::Read,
            Some("user") | Some("progress") => SessionStatus::Working,
            _ => SessionStatus::Working,
        };

        Some(DetectedStatus {
            status,
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

    fn resume_picker_args(&self) -> Vec<String> {
        vec!["--resume".into()]
    }
}

/// Encode a filesystem path to Claude's project directory name.
/// Every non-ASCII-alphanumeric character becomes `-`.
/// `/Users/nkyos/lab/tools/chatmux` → `-Users-nkyos-lab-tools-chatmux`
pub(crate) fn encode_project_path(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[derive(Deserialize)]
struct JsonlEntry {
    r#type: Option<String>,
    message: Option<MessagePart>,
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
                if block.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        texts.push(text.to_string());
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
                if block.get("type").and_then(|t| t.as_str()) == Some("text")
                    && let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        return text.to_string();
                    }
            }
            String::new()
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn encode_project_path_basic() {
        assert_eq!(
            encode_project_path("/Users/nkyos/lab/tools/chatmux"),
            "-Users-nkyos-lab-tools-chatmux"
        );
    }

    #[test]
    fn encode_project_path_underscores() {
        assert_eq!(
            encode_project_path("/Users/nkyos/my_project"),
            "-Users-nkyos-my-project"
        );
    }

    #[test]
    fn encode_project_path_dots() {
        assert_eq!(
            encode_project_path("/Users/nkyos/my.app"),
            "-Users-nkyos-my-app"
        );
    }

    #[test]
    fn encode_project_path_japanese() {
        // /事務/経費 → each non-ASCII char and / becomes '-'
        // / → -, 事 → -, 務 → -, / → -, 経 → -, 費 → - = 6 dashes
        assert_eq!(
            encode_project_path("/Users/nkyos/Documents/事務/経費"),
            "-Users-nkyos-Documents------"
        );
    }

    #[test]
    fn encode_project_path_spaces_and_hyphens() {
        assert_eq!(
            encode_project_path("/Users/nkyos/my project-v2"),
            "-Users-nkyos-my-project-v2"
        );
    }

    #[test]
    fn detect_status_end_turn() {
        let agent = ClaudeCodeAgent;
        let result = agent.detect_status(&fixture_path("end_turn.jsonl"));
        let detected = result.expect("should detect status");
        assert_eq!(detected.status, SessionStatus::Replied);
        assert_eq!(
            detected.last_prompt.as_deref(),
            Some("Hello, can you help me?")
        );
        assert_eq!(
            detected.last_reply.as_deref(),
            Some("Sure, I can help you with that.")
        );
    }

    #[test]
    fn detect_status_tool_use_working() {
        let agent = ClaudeCodeAgent;
        let result = agent.detect_status(&fixture_path("tool_use_mid.jsonl"));
        let detected = result.expect("should detect status");
        assert_eq!(detected.status, SessionStatus::Working);
        assert_eq!(
            detected.last_prompt.as_deref(),
            Some("Read the file src/main.rs")
        );
    }

    #[test]
    fn detect_status_clear_command() {
        let agent = ClaudeCodeAgent;
        let result = agent.detect_status(&fixture_path("clear_command.jsonl"));
        let detected = result.expect("should detect status");
        assert_eq!(detected.status, SessionStatus::Read);
    }

    #[test]
    fn detect_status_empty_file() {
        let agent = ClaudeCodeAgent;
        let result = agent.detect_status(&fixture_path("empty.jsonl"));
        assert!(result.is_none(), "empty file should return None");
    }

    #[test]
    fn detect_status_user_working() {
        let agent = ClaudeCodeAgent;
        let result = agent.detect_status(&fixture_path("user_working.jsonl"));
        let detected = result.expect("should detect status");
        assert_eq!(detected.status, SessionStatus::Working);
    }

    #[test]
    fn detect_status_progress_working() {
        let agent = ClaudeCodeAgent;
        let result = agent.detect_status(&fixture_path("progress_working.jsonl"));
        let detected = result.expect("should detect status");
        assert_eq!(detected.status, SessionStatus::Working);
    }

    #[test]
    fn detect_status_nonexistent_file() {
        let agent = ClaudeCodeAgent;
        let result = agent.detect_status(&fixture_path("does_not_exist.jsonl"));
        assert!(result.is_none());
    }

    #[test]
    fn launch_args_with_session_id() {
        let agent = ClaudeCodeAgent;
        let args = agent.launch_args(Some("abc-123"));
        assert_eq!(args, vec!["--session-id", "abc-123", "--dangerously-skip-permissions"]);
    }

    #[test]
    fn launch_args_without_session_id() {
        let agent = ClaudeCodeAgent;
        let args = agent.launch_args(None);
        assert_eq!(args, vec!["--dangerously-skip-permissions"]);
    }

    #[test]
    fn session_file_for_returns_correct_path() {
        let agent = ClaudeCodeAgent;
        let result = agent.session_file_for("/Users/nkyos/lab/tools/chatmux", "test-uuid-123");
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(path.to_string_lossy().contains("-Users-nkyos-lab-tools-chatmux"));
        assert!(path.to_string_lossy().ends_with("test-uuid-123.jsonl"));
    }
}
