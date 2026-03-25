use anyhow::{Context, Result};
use std::process::Command;

const SESSION_PREFIX: &str = "chatmux-";

pub struct TmuxClient;

impl TmuxClient {
    pub fn new() -> Self {
        Self
    }

    /// Create a new tmux session running `claude` in the given directory.
    pub fn new_session(
        &self,
        session_name: &str,
        cwd: &str,
        width: u16,
        height: u16,
    ) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let status = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                &full_name,
                "-x",
                &width.to_string(),
                "-y",
                &height.to_string(),
                "-c",
                cwd,
                "claude",
            ])
            .status()
            .context("Failed to run tmux")?;

        if !status.success() {
            anyhow::bail!("tmux new-session failed with {status}");
        }
        Ok(())
    }

    /// Capture the current pane content with ANSI escape sequences.
    /// Uses -J to join wrapped lines so ratatui can re-wrap correctly.
    pub fn capture_pane(&self, session_name: &str) -> Result<String> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["capture-pane", "-e", "-p", "-J", "-t", &full_name])
            .output()
            .context("Failed to run tmux capture-pane")?;

        if !output.status.success() {
            anyhow::bail!(
                "tmux capture-pane failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Send keys to a tmux session.
    pub fn send_keys(&self, session_name: &str, keys: &str) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let status = Command::new("tmux")
            .args(["send-keys", "-t", &full_name, keys])
            .status()
            .context("Failed to run tmux send-keys")?;

        if !status.success() {
            anyhow::bail!("tmux send-keys failed with {status}");
        }
        Ok(())
    }

    /// Send a literal key (e.g. "Enter", "Escape", "C-c") to a tmux session.
    pub fn send_key_literal(&self, session_name: &str, key: &str) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let status = Command::new("tmux")
            .args(["send-keys", "-t", &full_name, "-l", key])
            .status()
            .context("Failed to run tmux send-keys -l")?;

        if !status.success() {
            anyhow::bail!("tmux send-keys -l failed with {status}");
        }
        Ok(())
    }

    /// Kill a tmux session.
    pub fn kill_session(&self, session_name: &str) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let status = Command::new("tmux")
            .args(["kill-session", "-t", &full_name])
            .status()
            .context("Failed to run tmux kill-session")?;

        if !status.success() {
            anyhow::bail!("tmux kill-session failed with {status}");
        }
        Ok(())
    }

    /// List all chatmux-managed tmux sessions that are still alive.
    /// Returns session names without the "chatmux-" prefix.
    pub fn list_chatmux_sessions(&self) -> Vec<String> {
        let output = Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}"])
            .output();

        let Ok(output) = output else {
            return Vec::new();
        };

        if !output.status.success() {
            return Vec::new();
        }

        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix(SESSION_PREFIX))
            .map(|s| s.to_string())
            .collect()
    }

    /// Get the current working directory of a tmux session's pane.
    pub fn get_pane_cwd(&self, session_name: &str) -> Option<String> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["display-message", "-t", &full_name, "-p", "#{pane_current_path}"])
            .output()
            .ok()?;
        if output.status.success() {
            let cwd = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if cwd.is_empty() { None } else { Some(cwd) }
        } else {
            None
        }
    }

    /// Check if a tmux session is still alive.
    pub fn has_session(&self, session_name: &str) -> bool {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        Command::new("tmux")
            .args(["has-session", "-t", &full_name])
            .status()
            .is_ok_and(|s| s.success())
    }

    /// Resize the tmux pane to match the terminal view area.
    pub fn resize_pane(&self, session_name: &str, width: u16, height: u16) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let status = Command::new("tmux")
            .args([
                "resize-window",
                "-t",
                &full_name,
                "-x",
                &width.to_string(),
                "-y",
                &height.to_string(),
            ])
            .status()
            .context("Failed to run tmux resize-window")?;

        if !status.success() {
            anyhow::bail!("tmux resize-window failed with {status}");
        }
        Ok(())
    }
}
