use super::*;

impl App {
    pub(super) fn create_session(&mut self, path: &str, agent_kind: AgentKind) -> Result<()> {
        let agent = self.registry.get(agent_kind);
        let width = self.terminal.area.width.saturating_sub(2);
        let height = self.terminal.area.height.saturating_sub(2);
        let idx = self.manager.create(path, agent, width, height)?;
        self.select_by_index(idx);
        self.picker = None;
        self.focus = Focus::Terminal;
        if !self.cached_projects.contains(&path.to_string()) {
            self.cached_projects.insert(0, path.to_string());
        }
        self.manager.save_state();
        Ok(())
    }

    pub(super) fn resume_in_project(&mut self, path: &str, agent_kind: AgentKind) -> Result<()> {
        let agent = self.registry.get(agent_kind);
        let width = self.terminal.area.width.saturating_sub(2);
        let height = self.terminal.area.height.saturating_sub(2);
        let idx = self.manager.create_resume_picker(path, agent, width, height)?;
        self.select_by_index(idx);
        self.picker = None;
        self.focus = Focus::Terminal;
        if !self.cached_projects.contains(&path.to_string()) {
            self.cached_projects.insert(0, path.to_string());
        }
        Ok(())
    }

    /// Open the project directory in the configured editor.
    pub(super) fn open_editor(&self, cwd: &str) -> Result<()> {
        let (program, args) = self.config.editor_command_parts();

        std::process::Command::new(&program)
            .args(&args)
            .arg(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok();
        Ok(())
    }

    /// Execute the confirmed action (called from input handler).
    pub(super) fn execute_confirmed_action(&mut self) -> Result<()> {
        let action = self.confirm_action.take();
        match action {
            Some(ConfirmAction::UpgradeAndRestart) => self.do_upgrade_and_restart(),
            Some(ConfirmAction::RestartAll) => self.do_restart_all(),
            Some(ConfirmAction::DeleteSession { name }) => {
                let index = self.manager.sessions().iter().position(|s| s.name == name);
                if let Some(index) = index {
                    self.manager.remove(index)?;
                }
                match &self.sidebar.view {
                    SidebarView::ProjectSessions(cwd) => {
                        let cwd = cwd.clone();
                        let visible = self.project_session_indices(&cwd);
                        if visible.is_empty() {
                            self.selected = None;
                            self.terminal.content.clear();
                            self.sidebar.view = SidebarView::Projects;
                        } else {
                            self.select_by_index(visible[0]);
                        }
                    }
                    _ => {
                        if self.manager.is_empty() {
                            self.selected = None;
                            self.terminal.content.clear();
                        } else if self.selected_index().is_none() {
                            self.select_by_index(0);
                        }
                    }
                }
                Ok(())
            }
            Some(ConfirmAction::DeleteHistoryEntry { index }) => {
                if index < self.sidebar.history_entries.len() {
                    self.sidebar.history_entries.remove(index);
                    crate::session::state::save_history(&self.sidebar.history_entries);
                    if !self.sidebar.history_entries.is_empty() {
                        self.sidebar.history_selected =
                            self.sidebar.history_selected.min(self.sidebar.history_entries.len() - 1);
                    }
                }
                Ok(())
            }
            Some(ConfirmAction::OpenEditor { cwd }) => {
                self.open_editor(&cwd)
            }
            None => Ok(()),
        }
    }

    /// Cold-restore sessions from saved state when tmux sessions are gone (e.g. after reboot).
    pub(super) fn cold_restore_sessions(&mut self) -> Result<()> {
        let Some(saved) = crate::session::state::load() else {
            return Ok(());
        };

        let width = self.terminal.area.width.saturating_sub(2);
        let height = self.terminal.area.height.saturating_sub(2);

        for entry in &saved.sessions {
            if !std::path::Path::new(&entry.cwd).is_dir() {
                continue;
            }

            let agent = self.registry.get(entry.agent_kind);
            let idx = self.manager.create_resume(
                &entry.cwd,
                agent,
                entry.agent_session_id.as_deref(),
                width,
                height,
            )?;

            if let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                session.task_label = entry.task_label.clone();
                session.last_prompt = entry.last_prompt.clone();
                session.last_reply = entry.last_reply.clone();
                session.status = entry.status.as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(SessionStatus::Working);
                if entry.branch.is_some() {
                    session.branch = entry.branch.clone();
                }
                if let Some(epoch) = entry.last_activity_epoch {
                    session.set_activity_from_epoch(epoch);
                }
                if entry.created_epoch.is_some() {
                    session.created_epoch = entry.created_epoch;
                }
                if let Some(ref path_str) = entry.session_file {
                    let path = std::path::PathBuf::from(path_str);
                    if path.exists() {
                        session.jsonl_path = Some(path);
                    }
                }
            }
        }

        self.manager.ensure_next_id(saved.next_id);
        Ok(())
    }
}
