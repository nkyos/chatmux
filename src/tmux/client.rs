use anyhow::{Context, Result};
use std::process::Command;

const SESSION_PREFIX: &str = "chatmux-";

pub struct TmuxClient;

impl TmuxClient {
    pub fn new() -> Self {
        Self
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
        let full_name = format!("{SESSION_PREFIX}{session_name}");

        // Pass command + args as separate arguments so tmux uses direct
        // exec instead of /bin/sh -c, avoiding shell interpretation issues.
        // Locale env vars are forwarded via `env K=V` prefix.
        let mut cmd = Command::new("tmux");
        cmd.args([
            "-u", // Force UTF-8 mode
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
            "env",
        ]);
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

    /// Check if a tmux session is still alive.
    pub fn has_session(&self, session_name: &str) -> bool {
        let full_name = format!("{SESSION_PREFIX}{session_name}");
        Command::new("tmux")
            .args(["has-session", "-t", &full_name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
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
