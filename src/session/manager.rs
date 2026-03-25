use super::model::{Session, SessionStatus};
use crate::tmux::TmuxClient;
use anyhow::Result;

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

    /// Create a new session, launching claude in the given directory.
    pub fn create(&mut self, cwd: &str, width: u16, height: u16) -> Result<usize> {
        let id = self.next_id;
        self.next_id += 1;
        let name = format!("s{id}");

        self.tmux.new_session(&name, cwd, width, height)?;

        let session = Session::new(name, cwd.to_string());
        self.sessions.push(session);
        Ok(self.sessions.len() - 1)
    }

    /// Remove a session and kill its tmux session.
    pub fn remove(&mut self, index: usize) -> Result<()> {
        if index >= self.sessions.len() {
            anyhow::bail!("Session index out of range");
        }
        let session = self.sessions.remove(index);
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

    /// Capture the terminal output of a session.
    pub fn capture(&self, index: usize) -> Result<String> {
        let session = self
            .sessions
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Session index out of range"))?;
        self.tmux.capture_pane(&session.name)
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

    /// Update session status.
    pub fn set_status(&mut self, index: usize, status: SessionStatus) {
        if let Some(session) = self.sessions.get_mut(index) {
            session.status = status;
            session.last_activity = std::time::Instant::now();
        }
    }

    /// Kill all managed tmux sessions. Call on shutdown.
    pub fn cleanup(&mut self) {
        for session in &self.sessions {
            let _ = self.tmux.kill_session(&session.name);
        }
        self.sessions.clear();
    }
}
