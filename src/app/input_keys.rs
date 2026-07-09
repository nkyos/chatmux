use super::*;

impl App {
    pub(super) fn handle_startup_key(&mut self, code: KeyCode) -> Result<()> {
        let cold = matches!(self.mode, AppMode::Startup { cold_restore: true, .. });
        match code {
            KeyCode::Char('r') => {
                if cold {
                    // Cold restore: recreate tmux sessions from saved state.
                    self.cold_restore_sessions()?;
                } else {
                    // Normal restore from live tmux sessions.
                    self.manager.restore();
                }
                if !self.manager.is_empty() {
                    self.select_by_index(0);
                }
                // Save state immediately so crash recovery has current sessions.
                self.manager.save_state();
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('n') => {
                // Start fresh.
                if !cold {
                    self.manager.kill_all_chatmux_sessions();
                }
                crate::session::state::remove();
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_sidebar_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        match code {
            KeyCode::Char('q') => {
                self.detach_on_quit = true;
                self.should_quit = true;
            }
            KeyCode::Char('Q') => {
                self.detach_on_quit = false;
                self.should_quit = true;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.detach_on_quit = true;
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Char('J') => {
                if self.sidebar.sort_mode == SortMode::Manual
                    && self.sidebar.filter_input.is_none()
                    && let Some(idx) = self.selected_index()
                    && idx + 1 < self.manager.len()
                {
                    self.manager.sessions_mut().swap(idx, idx + 1);
                    self.select_by_index(idx + 1);
                }
            }
            KeyCode::Char('K') => {
                if self.sidebar.sort_mode == SortMode::Manual
                    && self.sidebar.filter_input.is_none()
                    && let Some(idx) = self.selected_index()
                    && idx > 0
                {
                    self.manager.sessions_mut().swap(idx, idx - 1);
                    self.select_by_index(idx - 1);
                }
            }
            KeyCode::Enter => {
                if self.selected.is_some() {
                    self.sidebar.filter_input = None;
                    self.mark_selected_as_read();
                    self.focus = Focus::Terminal;
                }
            }
            KeyCode::Char('n') => {
                let available: Vec<AgentKind> = self
                    .registry
                    .available()
                    .iter()
                    .map(|a| a.kind())
                    .collect();
                self.picker = Some(ProjectPicker::new(available, self.cached_projects.clone()));
                self.focus = Focus::ProjectPicker;
            }
            KeyCode::Char('e') => {
                if let Some(idx) = self.selected_index()
                    && let Some(session) = self.manager.get(idx) {
                        self.confirm_action = Some(ConfirmAction::OpenEditor { cwd: session.cwd.clone() });
                    }
            }
            KeyCode::Char('d') => {
                if let Some(name) = self.selected.clone() {
                    self.confirm_action = Some(ConfirmAction::DeleteSession { name });
                }
            }
            KeyCode::Char('r') => {
                if let Some(idx) = self.selected_index()
                    && let Some(session) = self.manager.get(idx) {
                        let current = session.task_label.clone().unwrap_or_default();
                        self.sidebar.rename_buf = Some(current);
                    }
            }
            KeyCode::Char('s') => {
                self.sidebar.sort_mode = self.sidebar.sort_mode.next();
                self.auto_sort();
            }
            KeyCode::Char('/') => {
                self.sidebar.filter_input = Some(String::new());
            }
            KeyCode::Char('h') => {
                self.sidebar.show_history = true;
                self.sidebar.history_entries = crate::session::state::load_history();
                self.sidebar.history_selected = 0;
            }
            KeyCode::Char('p') => {
                self.sidebar.view = SidebarView::Projects;
                self.sidebar.project_selected = 0;
            }
            KeyCode::Char('x') => {
                if let Some(idx) = self.selected_index()
                    && let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                        session.jsonl_stamp = None;
                    }
                self.last_status_poll = Instant::now() - self.config.polling.full_interval();
            }
            KeyCode::Char('X') => {
                if let Some(idx) = self.selected_index()
                    && let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                        session.jsonl_path = None;
                        session.jsonl_stamp = None;
                        session.agent_session_id = None;
                    }
                self.last_status_poll = Instant::now() - self.config.polling.full_interval();
            }
            KeyCode::Char('U') => {
                self.confirm_action = Some(ConfirmAction::UpgradeAndRestart);
            }
            KeyCode::Char('R') if !self.manager.is_empty() => {
                self.confirm_action = Some(ConfirmAction::RestartAll);
            }
            KeyCode::Char('?') => {
                self.show_help = true;
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle key events in project list and project-sessions views.
    pub(super) fn handle_project_view_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        match &self.sidebar.view {
            SidebarView::Projects => {
                match code {
                    KeyCode::Char('q') => {
                        self.detach_on_quit = true;
                        self.should_quit = true;
                    }
                    KeyCode::Char('Q') => {
                        self.detach_on_quit = false;
                        self.should_quit = true;
                    }
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        self.detach_on_quit = true;
                        self.should_quit = true;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let count = self.build_project_summaries().len();
                        if count > 0 {
                            self.sidebar.project_selected =
                                (self.sidebar.project_selected + 1).min(count - 1);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.sidebar.project_selected = self.sidebar.project_selected.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        let summaries = self.build_project_summaries();
                        if let Some(proj) = summaries.get(self.sidebar.project_selected) {
                            let cwd = proj.cwd.clone();
                            let indices = self.project_session_indices(&cwd);
                            self.sidebar.view = SidebarView::ProjectSessions(cwd);
                            if let Some(&idx) = indices.first() {
                                self.select_by_index(idx);
                            }
                            self.terminal.scroll = 0;
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('p') => {
                        self.sidebar.view = SidebarView::Sessions;
                    }
                    KeyCode::Char('n') => {
                        let available: Vec<AgentKind> = self
                            .registry
                            .available()
                            .iter()
                            .map(|a| a.kind())
                            .collect();
                        self.picker =
                            Some(ProjectPicker::new(available, self.cached_projects.clone()));
                        self.focus = Focus::ProjectPicker;
                    }
                    KeyCode::Char('U') => {
                        self.confirm_action = Some(ConfirmAction::UpgradeAndRestart);
                    }
                    KeyCode::Char('R') if !self.manager.is_empty() => {
                        self.confirm_action = Some(ConfirmAction::RestartAll);
                    }
                    KeyCode::Char('?') => {
                        self.show_help = true;
                    }
                    _ => {}
                }
            }
            SidebarView::ProjectSessions(cwd) => {
                let cwd_owned = cwd.clone();
                match code {
                    KeyCode::Char('q') => {
                        self.detach_on_quit = true;
                        self.should_quit = true;
                    }
                    KeyCode::Char('Q') => {
                        self.detach_on_quit = false;
                        self.should_quit = true;
                    }
                    KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                        self.detach_on_quit = true;
                        self.should_quit = true;
                    }
                    KeyCode::Esc => {
                        self.sidebar.view = SidebarView::Projects;
                    }
                    KeyCode::Char('p') => {
                        self.sidebar.view = SidebarView::Sessions;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let visible = self.project_session_indices(&cwd_owned);
                        self.select_next_in(&visible);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let visible = self.project_session_indices(&cwd_owned);
                        self.select_prev_in(&visible);
                    }
                    KeyCode::Enter => {
                        if self.selected.is_some() {
                            self.mark_selected_as_read();
                            self.focus = Focus::Terminal;
                        }
                    }
                    KeyCode::Char('d') => {
                        if let Some(name) = self.selected.clone() {
                            self.confirm_action = Some(ConfirmAction::DeleteSession { name });
                        }
                    }
                    KeyCode::Char('r') => {
                        if let Some(idx) = self.selected_index()
                            && let Some(session) = self.manager.get(idx) {
                                let current =
                                    session.task_label.clone().unwrap_or_default();
                                self.sidebar.rename_buf = Some(current);
                            }
                    }
                    KeyCode::Char('n') => {
                        let available: Vec<AgentKind> = self
                            .registry
                            .available()
                            .iter()
                            .map(|a| a.kind())
                            .collect();
                        self.picker =
                            Some(ProjectPicker::new(available, self.cached_projects.clone()));
                        self.focus = Focus::ProjectPicker;
                    }
                    KeyCode::Char('e') => {
                        if let Some(idx) = self.selected_index()
                            && let Some(session) = self.manager.get(idx) {
                                self.confirm_action = Some(ConfirmAction::OpenEditor { cwd: session.cwd.clone() });
                            }
                    }
                    KeyCode::Char('U') => {
                        self.confirm_action = Some(ConfirmAction::UpgradeAndRestart);
                    }
                    KeyCode::Char('R') if !self.manager.is_empty() => {
                        self.confirm_action = Some(ConfirmAction::RestartAll);
                    }
                    KeyCode::Char('?') => {
                        self.show_help = true;
                    }
                    _ => {}
                }
            }
            SidebarView::Sessions => {}
        }
        Ok(())
    }

    pub(super) fn handle_confirm_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Char('y') | KeyCode::Enter => {
                self.execute_confirmed_action()?;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.confirm_action = None;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_rename_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Char(c) => {
                if let Some(ref mut buf) = self.sidebar.rename_buf {
                    buf.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut buf) = self.sidebar.rename_buf {
                    buf.pop();
                }
            }
            KeyCode::Enter => {
                if let (Some(idx), Some(buf)) = (self.selected_index(), self.sidebar.rename_buf.take())
                    && let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                        session.task_label = if buf.is_empty() { None } else { Some(buf) };
                    }
            }
            KeyCode::Esc => {
                self.sidebar.rename_buf = None;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_history_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        match code {
            KeyCode::Char('h') | KeyCode::Esc => {
                self.sidebar.show_history = false;
            }
            KeyCode::Char('q') => {
                self.detach_on_quit = true;
                self.should_quit = true;
            }
            KeyCode::Char('Q') => {
                self.detach_on_quit = false;
                self.should_quit = true;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.detach_on_quit = true;
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.sidebar.history_entries.is_empty() {
                    self.sidebar.history_selected =
                        (self.sidebar.history_selected + 1).min(self.sidebar.history_entries.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.sidebar.history_selected = self.sidebar.history_selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(entry) = self.sidebar.history_entries.get(self.sidebar.history_selected) {
                    let cwd = entry.cwd.clone();
                    let agent_kind = entry.agent_kind;
                    self.sidebar.show_history = false;
                    self.create_session(&cwd, agent_kind)?;
                }
            }
            KeyCode::Char('d') => {
                if self.sidebar.history_selected < self.sidebar.history_entries.len() {
                    self.confirm_action = Some(ConfirmAction::DeleteHistoryEntry { index: self.sidebar.history_selected });
                }
            }
            KeyCode::Char('?') => {
                self.show_help = true;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_terminal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        // Search input mode: typing a pattern.
        if let Some(ref mut buf) = self.terminal.search_input {
            match code {
                KeyCode::Char(c) => {
                    buf.push(c);
                    return Ok(());
                }
                KeyCode::Backspace => {
                    buf.pop();
                    return Ok(());
                }
                KeyCode::Enter => {
                    let pattern = buf.clone();
                    self.terminal.search_input = None;
                    if !pattern.is_empty() {
                        self.execute_search(&pattern);
                    }
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.terminal.search_input = None;
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }

        // Search results active: n/N/Esc navigate or dismiss.
        if self.terminal.search.is_some() {
            match code {
                KeyCode::Char('n') => {
                    self.search_next();
                    return Ok(());
                }
                KeyCode::Char('N') => {
                    self.search_prev();
                    return Ok(());
                }
                KeyCode::Esc => {
                    self.terminal.search = None;
                    self.terminal.scroll = 0;
                    return Ok(());
                }
                _ => {
                    self.terminal.search = None;
                }
            }
        }

        // When selection is active, handle copy keys before forwarding to tmux.
        if let Some(sel) = self.terminal.selection
            && sel.start != sel.end {
                let is_copy = matches!(code, KeyCode::Char('y'))
                    || (matches!(code, KeyCode::Char('c'))
                        && (modifiers.contains(KeyModifiers::CONTROL)
                            || modifiers.contains(KeyModifiers::SUPER)));
                if is_copy {
                    self.copy_selection_to_clipboard(&sel);
                    self.terminal.selection = None;
                    return Ok(());
                }
                if matches!(code, KeyCode::Esc) {
                    self.terminal.selection = None;
                    return Ok(());
                }
            }

        // Any key input in terminal clears selection and resets scroll.
        self.terminal.selection = None;
        self.terminal.scroll = 0;

        // Prefix mode: Ctrl+] was pressed, interpret next key as a command.
        if self.terminal.prefix_active {
            self.terminal.prefix_active = false;
            match code {
                KeyCode::Esc | KeyCode::Char('[') => {
                    self.focus = Focus::Sidebar;
                    return Ok(());
                }
                _ if is_prefix_key(code, modifiers) => {
                    if let Some(idx) = self.selected_index() {
                        let _ = self.manager.send_keys(idx, "C-]");
                    }
                    return Ok(());
                }
                KeyCode::Char('?') => {
                    self.show_help = true;
                    return Ok(());
                }
                KeyCode::Char('/') => {
                    self.terminal.search_input = Some(String::new());
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }

        // Ctrl+] → enter prefix mode
        if is_prefix_key(code, modifiers) {
            self.terminal.prefix_active = true;
            return Ok(());
        }

        let Some(idx) = self.selected_index() else {
            return Ok(());
        };

        // Build tmux key name from crossterm KeyCode + modifiers.
        let base_name: Option<String> = match code {
            KeyCode::Char(c) => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    Some(format!("C-{c}"))
                } else if modifiers.contains(KeyModifiers::ALT) {
                    Some(format!("M-{c}"))
                } else {
                    if let Some(session) = self.manager.get(idx) {
                        let _ = self.manager
                            .tmux()
                            .send_key_literal(&session.name, &c.to_string());
                    }
                    None
                }
            }
            KeyCode::Enter => Some("Enter".into()),
            KeyCode::Backspace => Some("BSpace".into()),
            KeyCode::Tab => Some("Tab".into()),
            KeyCode::BackTab => Some("BTab".into()),
            KeyCode::Up => Some("Up".into()),
            KeyCode::Down => Some("Down".into()),
            KeyCode::Left => Some("Left".into()),
            KeyCode::Right => Some("Right".into()),
            KeyCode::Home => Some("Home".into()),
            KeyCode::End => Some("End".into()),
            KeyCode::PageUp => Some("PageUp".into()),
            KeyCode::PageDown => Some("PageDown".into()),
            KeyCode::Delete => Some("DC".into()),
            KeyCode::Insert => Some("IC".into()),
            KeyCode::F(n) => Some(format!("F{n}")),
            KeyCode::Esc => Some("Escape".into()),
            _ => None,
        };

        if let Some(mut key) = base_name {
            if !matches!(code, KeyCode::Char(_)) {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    key = format!("C-{key}");
                }
                if modifiers.contains(KeyModifiers::ALT) {
                    key = format!("M-{key}");
                }
                if modifiers.contains(KeyModifiers::SHIFT) {
                    key = format!("S-{key}");
                }
            }
            let _ = self.manager.send_keys(idx, &key);
        }
        Ok(())
    }

    /// Execute a scrollback search.
    fn execute_search(&mut self, pattern: &str) {
        let Some(idx) = self.selected_index() else {
            return;
        };
        let Ok(history) = self.manager.capture_history_plain(idx) else {
            return;
        };

        // Smart-case: case-insensitive if pattern is all lowercase.
        let case_insensitive = pattern.chars().all(|c| !c.is_uppercase());
        let pat = if case_insensitive {
            pattern.to_lowercase()
        } else {
            pattern.to_string()
        };

        let lines: Vec<&str> = history.lines().collect();
        let total = lines.len();
        let mut matches: Vec<u16> = Vec::new();

        for (i, line) in lines.iter().enumerate() {
            let haystack = if case_insensitive {
                line.to_lowercase()
            } else {
                line.to_string()
            };
            if haystack.contains(&pat) {
                let offset_from_bottom = (total - 1 - i) as u16;
                matches.push(offset_from_bottom);
            }
        }

        // matches[0] is the closest to the bottom (most recent).
        matches.reverse();

        if matches.is_empty() {
            return;
        }

        let scroll = matches[0];
        self.terminal.scroll = scroll;
        self.terminal.search = Some(super::SearchState {
            pattern: pattern.to_string(),
            matches,
            current: 0,
        });
    }

    fn search_next(&mut self) {
        if let Some(ref mut search) = self.terminal.search
            && search.current + 1 < search.matches.len()
        {
            search.current += 1;
            self.terminal.scroll = search.matches[search.current];
        }
    }

    fn search_prev(&mut self) {
        if let Some(ref mut search) = self.terminal.search
            && search.current > 0
        {
            search.current -= 1;
            self.terminal.scroll = search.matches[search.current];
        }
    }

    pub(super) fn handle_picker_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        // Ctrl+R: resume in the selected project.
        if code == KeyCode::Char('r') && modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(ref picker) = self.picker {
                let path = match picker.mode {
                    PickerMode::RecentProjects => {
                        let filtered = picker.filtered_recent_projects();
                        filtered.get(picker.selected).cloned()
                    }
                    PickerMode::DirectoryBrowser => {
                        Some(picker.browser_cwd.clone())
                    }
                    PickerMode::AgentSelect { ref path } => {
                        Some(path.clone())
                    }
                };
                if let Some(path) = path {
                    let agent_kind = picker
                        .available_agents
                        .first()
                        .copied()
                        .unwrap_or_default();
                    return self.resume_in_project(&path, agent_kind);
                }
            }
            return Ok(());
        }

        let Some(ref mut picker) = self.picker else {
            return Ok(());
        };

        match code {
            KeyCode::Esc => {
                if picker.has_filter() {
                    picker.clear_filter();
                } else if matches!(picker.mode, PickerMode::AgentSelect { .. }) {
                    picker.back_from_agent_select();
                } else if picker.mode == PickerMode::DirectoryBrowser {
                    picker.back_to_recent();
                } else {
                    self.picker = None;
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Down => picker.move_down(),
            KeyCode::Up => picker.move_up(),
            KeyCode::Enter => {
                if let Some(result) = picker.confirm() {
                    self.create_session(&result.path, result.agent_kind)?;
                }
            }
            KeyCode::Char(' ') => {
                if picker.mode == PickerMode::DirectoryBrowser && !picker.has_filter() {
                    if let Some(result) = picker.select_current_dir() {
                        self.create_session(&result.path, result.agent_kind)?;
                    }
                } else {
                    picker.on_char(' ');
                }
            }
            KeyCode::Backspace => {
                if !picker.on_backspace_filter()
                    && picker.mode == PickerMode::DirectoryBrowser {
                        picker.go_up();
                    }
            }
            KeyCode::Char(c) => {
                picker.on_char(c);
            }
            _ => {}
        }
        Ok(())
    }
}
