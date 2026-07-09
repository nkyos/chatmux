use anyhow::{Context, Result};
use std::io::Write;
use std::process::Command;

const SESSION_PREFIX: &str = "chatmux-";

pub struct TmuxClient {
    has_direnv: bool,
}

impl TmuxClient {
    pub fn new() -> Self {
        let has_direnv = Command::new("direnv")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        Self { has_direnv }
    }

    /// Create a new tmux session running the given command in the given directory.
    pub fn new_session(
        &self,
        session_name: &str,
        cwd: &str,
        command: &str,
        args: &[String],
        width: u16,
        height: u16,
    ) -> Result<()> {
        self.new_session_with_env(session_name, cwd, command, args, width, height, &[])
    }

    /// Create a new tmux session with extra environment variables.
    #[allow(clippy::too_many_arguments)]
    pub fn new_session_with_env(
        &self,
        session_name: &str,
        cwd: &str,
        command: &str,
        args: &[String],
        width: u16,
        height: u16,
        extra_env: &[(&str, &str)],
    ) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");

        let mut cmd = Command::new("tmux");
        cmd.args([
            "-u",
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
        ]);
        for (key, val) in extra_env {
            cmd.args(["-e", &format!("{key}={val}")]);
        }
        if self.has_direnv {
            cmd.args(["direnv", "exec", cwd]);
        }
        cmd.arg("env");
        for (key, val) in Self::locale_env_pairs() {
            cmd.arg(format!("{key}={val}"));
        }
        cmd.arg(command);
        cmd.args(args);

        let status = cmd
            .status()
            .context("Failed to run tmux")?;

        if !status.success() {
            anyhow::bail!("tmux new-session failed with {status}");
        }

        // Apply sensible defaults for the session.
        self.configure_session(&full_name);

        Ok(())
    }

    /// Collect locale/terminal env vars as key-value pairs for safe forwarding.
    pub fn locale_env_pairs() -> Vec<(String, String)> {
        ["LANG", "LC_ALL", "LC_CTYPE", "TERM"]
            .into_iter()
            .filter_map(|k| {
                std::env::var(k)
                    .ok()
                    .filter(|v| !v.is_empty())
                    .map(|v| (k.to_string(), v))
            })
            .collect()
    }

    /// Apply sensible tmux options to a session (mouse, scrollback).
    fn configure_session(&self, full_name: &str) {
        let options: &[(&str, &str)] = &[
            ("mouse", "on"),
            ("history-limit", "50000"),
        ];
        for (key, value) in options {
            let _ = Command::new("tmux")
                .args(["set-option", "-t", full_name, key, value])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    /// Capture the current pane content with ANSI escape sequences.
    /// Uses -J to join wrapped lines so ratatui can re-wrap correctly.
    pub fn capture_pane(&self, session_name: &str) -> Result<String> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["capture-pane", "-e", "-p", "-t", &full_name])
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

    /// Capture pane content shifted up by `scroll_back` lines into scrollback history.
    /// Returns a window of `pane_height` lines starting from `scroll_back` lines above
    /// the current visible region.
    pub fn capture_pane_scroll(
        &self,
        session_name: &str,
        scroll_back: u16,
        pane_height: u16,
    ) -> Result<String> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let start = -(scroll_back as i32);
        let end = start + (pane_height as i32) - 1;
        let output = Command::new("tmux")
            .args([
                "capture-pane",
                "-e",
                "-p",
                "-S",
                &start.to_string(),
                "-E",
                &end.to_string(),
                "-t",
                &full_name,
            ])
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
        let output = Command::new("tmux")
            .args(["send-keys", "-t", &full_name, keys])
            .stderr(std::process::Stdio::piped())
            .output()
            .context("Failed to run tmux send-keys")?;

        if !output.status.success() {
            anyhow::bail!(
                "tmux send-keys failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Send a literal key (e.g. "Enter", "Escape", "C-c") to a tmux session.
    pub fn send_key_literal(&self, session_name: &str, key: &str) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["send-keys", "-t", &full_name, "-l", key])
            .stderr(std::process::Stdio::piped())
            .output()
            .context("Failed to run tmux send-keys -l")?;

        if !output.status.success() {
            anyhow::bail!(
                "tmux send-keys -l failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Paste text into a tmux session via load-buffer/paste-buffer.
    /// Unlike send_key_literal, this handles arbitrarily long text.
    pub fn paste_text(&self, session_name: &str, text: &str) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let buf_name = "chatmux-paste";

        // Load text into a named tmux buffer via stdin (no arg length limit).
        let mut child = Command::new("tmux")
            .args(["load-buffer", "-b", buf_name, "-"])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("Failed to spawn tmux load-buffer")?;

        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .context("Failed to write to tmux load-buffer stdin")?;

        let output = child
            .wait_with_output()
            .context("Failed to wait for tmux load-buffer")?;

        if !output.status.success() {
            anyhow::bail!(
                "tmux load-buffer failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Paste from the buffer into the target pane.
        let output = Command::new("tmux")
            .args(["paste-buffer", "-b", buf_name, "-t", &full_name, "-d"])
            .stderr(std::process::Stdio::piped())
            .output()
            .context("Failed to run tmux paste-buffer")?;

        if !output.status.success() {
            anyhow::bail!(
                "tmux paste-buffer failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
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

    /// Get the number of lines in the scrollback history for a session's pane.
    pub fn history_size(&self, session_name: &str) -> u16 {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args([
                "display-message",
                "-t",
                &full_name,
                "-p",
                "#{history_size}",
            ])
            .output()
            .ok();
        output
            .filter(|o| o.status.success())
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u16>()
                    .ok()
            })
            .unwrap_or(0)
    }

    /// Check if a tmux session has any attached clients.
    /// Returns true if someone is directly attached (e.g. via `chatmux claude`).
    pub fn has_attached_client(&self, session_name: &str) -> bool {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["list-clients", "-t", &full_name, "-F", "#{client_name}"])
            .output()
            .ok();
        output
            .filter(|o| o.status.success())
            .is_some_and(|o| !o.stdout.is_empty())
    }

    /// Get the original start command of a tmux session's pane.
    /// This is the full command string passed to `new-session`, which
    /// stays constant even when a child process is in the foreground.
    pub fn get_pane_start_command(&self, session_name: &str) -> Option<String> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["display-message", "-t", &full_name, "-p", "#{pane_start_command}"])
            .output()
            .ok()?;
        if output.status.success() {
            let cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if cmd.is_empty() { None } else { Some(cmd) }
        } else {
            None
        }
    }

    /// Get the current command running in a tmux session's pane.
    pub fn get_pane_command(&self, session_name: &str) -> Option<String> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["display-message", "-t", &full_name, "-p", "#{pane_current_command}"])
            .output()
            .ok()?;
        if output.status.success() {
            let cmd = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if cmd.is_empty() { None } else { Some(cmd) }
        } else {
            None
        }
    }

    /// Get the creation time (Unix epoch) of a tmux session.
    pub fn get_session_created(&self, session_name: &str) -> Option<u64> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["display-message", "-t", &full_name, "-p", "#{session_created}"])
            .output()
            .ok()?;
        if output.status.success() {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        } else {
            None
        }
    }

    /// Check if the pane in a tmux session has exited (process dead, pane remains).
    pub fn is_pane_dead(&self, session_name: &str) -> bool {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args(["display-message", "-t", &full_name, "-p", "#{pane_dead}"])
            .output()
            .ok();
        output
            .filter(|o| o.status.success())
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
    }

    /// Create a new tmux session with remain-on-exit (for upgrade sessions).
    pub fn new_session_with_remain_on_exit(
        &self,
        session_name: &str,
        cwd: &str,
        command: &str,
        args: &[String],
        width: u16,
        height: u16,
    ) -> Result<()> {
        self.new_session(session_name, cwd, command, args, width, height)?;
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let _ = Command::new("tmux")
            .args(["set-option", "-t", &full_name, "remain-on-exit", "on"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        Ok(())
    }

    /// Detect the agent kind running in a tmux session by inspecting
    /// the pane's current command and start command.
    pub fn detect_agent_kind(&self, session_name: &str) -> Option<crate::agent::AgentKind> {
        use crate::agent::AgentKind;
        if let Some(cmd) = self.get_pane_command(session_name) {
            match cmd.as_str() {
                "claude" => return Some(AgentKind::ClaudeCode),
                "codex" => return Some(AgentKind::Codex),
                _ => {}
            }
        }
        if let Some(start_cmd) = self.get_pane_start_command(session_name) {
            if start_cmd.contains("claude") {
                return Some(AgentKind::ClaudeCode);
            }
            if start_cmd.contains("codex") {
                return Some(AgentKind::Codex);
            }
        }
        None
    }

    /// Resize the tmux pane to match the terminal view area.
    pub fn resize_pane(&self, session_name: &str, width: u16, height: u16) -> Result<()> {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        let output = Command::new("tmux")
            .args([
                "resize-window",
                "-t",
                &full_name,
                "-x",
                &width.to_string(),
                "-y",
                &height.to_string(),
            ])
            .stderr(std::process::Stdio::piped())
            .output()
            .context("Failed to run tmux resize-window")?;

        if !output.status.success() {
            anyhow::bail!(
                "tmux resize-window failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }
}
