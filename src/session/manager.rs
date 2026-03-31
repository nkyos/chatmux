use super::model::{Session, SessionStatus};
use super::state::{self, SavedState, SessionEntry};
use crate::agent::{Agent, AgentKind};
use crate::tmux::TmuxClient;
use anyhow::Result;
use std::collections::HashSet;

pub struct SessionManager {
    sessions: Vec<Session>,
    tmux: TmuxClient,
    next_id: usize,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            tmux: TmuxClient::new(),
            next_id: 0,
        }
    }

    pub fn tmux(&self) -> &TmuxClient {
        &self.tmux
    }

    /// Create a new session, launching the given agent in the given directory.
    pub fn create(
        &mut self,
        cwd: &str,
        agent: &dyn Agent,
        width: u16,
        height: u16,
    ) -> Result<usize> {
        // Snapshot existing session files BEFORE launching the agent.
        // This lets us later identify which new file belongs to this session.
        let pre_existing = agent.list_session_files(cwd);

        let id = self.next_id;
        self.next_id += 1;
        let name = format!("s{id}");

        self.tmux
            .new_session(&name, cwd, agent.command(), &agent.args(), width, height)?;

        let mut session = Session::new(name, cwd.to_string(), agent.kind());
        session.pre_existing_files = pre_existing;
        self.sessions.push(session);
        Ok(self.sessions.len() - 1)
    }

    /// Remove a session and kill its tmux session.
    pub fn remove(&mut self, index: usize) -> Result<()> {
        if index >= self.sessions.len() {
            anyhow::bail!("Session index out of range");
        }
        let session = self.sessions.remove(index);
        // Record in history before killing.
        let entry = state::HistoryEntry {
            cwd: session.cwd.clone(),
            project_name: session.project_name.clone(),
            agent_kind: session.agent_kind,
            task_label: session.task_label.clone(),
            last_prompt: session.last_prompt.clone(),
            ended_at: std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        state::append_history(&entry);
        // Best-effort kill; ignore errors if already dead.
        let _ = self.tmux.kill_session(&session.name);
        Ok(())
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn sessions_mut(&mut self) -> &mut Vec<Session> {
        &mut self.sessions
    }

    pub fn get(&self, index: usize) -> Option<&Session> {
        self.sessions.get(index)
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    /// Ensure next_id is at least `min`, preventing ID collisions with externally created sessions.
    pub fn ensure_next_id(&mut self, min: usize) {
        if self.next_id < min {
            self.next_id = min;
        }
    }

    /// Sort sessions by smart ordering (waiting > replied > working > idle).
    pub fn sort_by_priority(&mut self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.sessions.len()).collect();
        indices.sort_by(|&a, &b| {
            let sa = &self.sessions[a];
            let sb = &self.sessions[b];
            sa.status
                .sort_priority()
                .cmp(&sb.status.sort_priority())
                .then(sb.last_activity.cmp(&sa.last_activity))
        });

        // Reorder sessions in-place based on sorted indices.
        let reordered: Vec<Session> = indices.iter().map(|&i| self.sessions[i].clone()).collect();
        self.sessions = reordered;
        indices
    }

    /// Sort sessions by last activity (most recent first).
    pub fn sort_by_activity(&mut self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..self.sessions.len()).collect();
        indices.sort_by(|&a, &b| {
            self.sessions[b]
                .last_activity
                .cmp(&self.sessions[a].last_activity)
        });
        let reordered: Vec<Session> = indices.iter().map(|&i| self.sessions[i].clone()).collect();
        self.sessions = reordered;
        indices
    }

    /// Capture the terminal output of a session.
    pub fn capture(&self, index: usize) -> Result<String> {
        let session = self
            .sessions
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Session index out of range"))?;
        self.tmux.capture_pane(&session.name)
    }

    /// Capture terminal output scrolled back by `scroll_back` lines.
    pub fn capture_scroll(
        &self,
        index: usize,
        scroll_back: u16,
        pane_height: u16,
    ) -> Result<String> {
        let session = self
            .sessions
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Session index out of range"))?;
        self.tmux
            .capture_pane_scroll(&session.name, scroll_back, pane_height)
    }

    /// Get the scrollback history size for a session.
    pub fn history_size(&self, index: usize) -> u16 {
        self.sessions
            .get(index)
            .map(|s| self.tmux.history_size(&s.name))
            .unwrap_or(0)
    }

    /// Send keys to a session.
    pub fn send_keys(&self, index: usize, keys: &str) -> Result<()> {
        let session = self
            .sessions
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Session index out of range"))?;
        self.tmux.send_keys(&session.name, keys)
    }

    /// Resize the tmux pane for a session.
    pub fn resize(&self, index: usize, width: u16, height: u16) -> Result<()> {
        let session = self
            .sessions
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Session index out of range"))?;
        self.tmux.resize_pane(&session.name, width, height)
    }

    /// Restore sessions from saved state + live tmux sessions.
    /// Only restores sessions whose tmux session is still alive.
    pub fn restore(&mut self) {
        let live: HashSet<String> = self.tmux.list_chatmux_sessions().into_iter().collect();

        if let Some(saved) = state::load() {
            // Restore from saved state, but only if tmux session is alive.
            for entry in saved.sessions {
                if live.contains(&entry.name) {
                    let mut session = Session::new(entry.name, entry.cwd, entry.agent_kind);
                    session.project_name = entry.project_name;
                    session.task_label = entry.task_label;
                    session.last_prompt = entry.last_prompt;
                    session.last_reply = entry.last_reply;
                    // Restore saved status (defaults to Working if missing).
                    session.status = match entry.status.as_deref() {
                        Some("replied") => SessionStatus::Replied,
                        Some("read") => SessionStatus::Read,
                        _ => SessionStatus::Working,
                    };
                    session.jsonl_path = entry
                        .session_file
                        .as_ref()
                        .map(std::path::PathBuf::from);
                    // Restore JSONL file stamp so the poll skips unchanged files.
                    session.jsonl_stamp = entry.jsonl_modified_epoch
                        .map(|epoch| {
                            let nsec = entry.jsonl_modified_nsec.unwrap_or(0);
                            crate::agent::FileStamp {
                                modified: Some(
                                    std::time::SystemTime::UNIX_EPOCH
                                        + std::time::Duration::new(epoch, nsec),
                                ),
                                len: entry.jsonl_len.unwrap_or(0),
                            }
                        });
                    if entry.branch.is_some() {
                        session.branch = entry.branch;
                    }
                    session.agent_session_id = entry.agent_session_id;
                    session.created_epoch = entry.created_epoch;
                    // Restore last activity from saved epoch or JSONL file mtime.
                    let file_epoch = session
                        .jsonl_path
                        .as_ref()
                        .and_then(|p| p.metadata().ok())
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs());
                    let saved_epoch = entry.last_activity_epoch;
                    // Use whichever is more recent: saved epoch or file mtime.
                    let epoch = match (saved_epoch, file_epoch) {
                        (Some(s), Some(f)) => s.max(f),
                        (Some(s), None) => s,
                        (None, Some(f)) => f,
                        (None, None) => 0,
                    };
                    if epoch > 0 {
                        session.set_activity_from_epoch(epoch);
                    }
                    self.sessions.push(session);
                }
            }
            self.next_id = saved.next_id;
        } else {
            // No state file — reconstruct from live tmux sessions.
            // Detect agent kind from tmux pane command.
            for name in &live {
                let cwd = self
                    .tmux
                    .get_pane_cwd(name)
                    .unwrap_or_else(|| "/".to_string());
                let agent_kind = self.detect_agent_from_tmux(name);
                let session = Session::new(name.clone(), cwd, agent_kind);
                self.sessions.push(session);
            }
        }

        // Ensure next_id is higher than any restored session.
        for session in &self.sessions {
            if let Some(num) = session
                .name
                .strip_prefix('s')
                .and_then(|n| n.parse::<usize>().ok())
            {
                self.next_id = self.next_id.max(num + 1);
            }
        }
    }

    /// Detect agent kind from tmux pane command.
    fn detect_agent_from_tmux(&self, name: &str) -> AgentKind {
        if let Some(cmd) = self.tmux.get_pane_command(name) {
            match cmd.as_str() {
                "claude" => return AgentKind::ClaudeCode,
                "codex" => return AgentKind::Codex,
                _ => {}
            }
        }
        if let Some(start_cmd) = self.tmux.get_pane_start_command(name) {
            if start_cmd.contains("claude") {
                return AgentKind::ClaudeCode;
            }
            if start_cmd.contains("codex") {
                return AgentKind::Codex;
            }
        }
        AgentKind::default()
    }

    /// Save current session state to disk.
    pub fn save_state(&self) {
        let saved = SavedState {
            sessions: self
                .sessions
                .iter()
                .map(|s| SessionEntry {
                    name: s.name.clone(),
                    cwd: s.cwd.clone(),
                    project_name: s.project_name.clone(),
                    agent_kind: s.agent_kind,
                    task_label: s.task_label.clone(),
                    last_prompt: s.last_prompt.clone(),
                    last_reply: s.last_reply.clone(),
                    session_file: s
                        .jsonl_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    last_activity_epoch: Some(s.last_activity_epoch),
                    status: Some(s.status.name().to_string()),
                    jsonl_modified_epoch: s.jsonl_stamp
                        .and_then(|st| st.modified)
                        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs()),
                    jsonl_modified_nsec: s.jsonl_stamp
                        .and_then(|st| st.modified)
                        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                        .map(|d| d.subsec_nanos()),
                    jsonl_len: s.jsonl_stamp.map(|st| st.len),
                    branch: s.branch.clone(),
                    agent_session_id: s.agent_session_id.clone(),
                    created_epoch: s.created_epoch,
                })
                .collect(),
            next_id: self.next_id,
        };
        let _ = state::save(&saved);
    }

    /// Resume a session with agent-specific resume flags.
    pub fn create_resume(
        &mut self,
        cwd: &str,
        agent: &dyn Agent,
        session_id: Option<&str>,
        width: u16,
        height: u16,
    ) -> Result<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let name = format!("s{id}");

        self.tmux.new_session(
            &name,
            cwd,
            agent.resume_command(),
            &agent.resume_args(session_id),
            width,
            height,
        )?;

        let mut session = Session::new(name, cwd.to_string(), agent.kind());
        session.agent_session_id = session_id.map(|s| s.to_string());
        self.sessions.push(session);
        Ok(self.sessions.len() - 1)
    }

    /// Create a session running the agent's interactive resume/session picker.
    pub fn create_resume_picker(
        &mut self,
        cwd: &str,
        agent: &dyn Agent,
        width: u16,
        height: u16,
    ) -> Result<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let name = format!("s{id}");

        self.tmux.new_session(
            &name,
            cwd,
            agent.resume_command(),
            &agent.resume_picker_args(),
            width,
            height,
        )?;

        let session = Session::new(name, cwd.to_string(), agent.kind());
        self.sessions.push(session);
        Ok(self.sessions.len() - 1)
    }

    /// Kill all chatmux tmux sessions (including orphaned ones not tracked by this manager).
    pub fn kill_all_chatmux_sessions(&self) {
        for name in self.tmux.list_chatmux_sessions() {
            let _ = self.tmux.kill_session(&name);
        }
    }

    /// Detach: save state and exit without killing tmux sessions.
    pub fn detach(&self) {
        self.save_state();
    }

    /// Kill all managed tmux sessions and remove state file. Call on full shutdown.
    pub fn cleanup(&mut self) {
        // Record all sessions in history before killing.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        for session in &self.sessions {
            let entry = state::HistoryEntry {
                cwd: session.cwd.clone(),
                project_name: session.project_name.clone(),
                agent_kind: session.agent_kind,
                task_label: session.task_label.clone(),
                last_prompt: session.last_prompt.clone(),
                ended_at: now,
            };
            state::append_history(&entry);
        }
        self.kill_all_chatmux_sessions();
        self.sessions.clear();
        state::remove();
    }
}
