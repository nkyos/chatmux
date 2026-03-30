use super::*;

impl App {
    pub fn handle_event(&mut self) -> Result<()> {
        if !event::poll(Duration::from_millis(100))? {
            return Ok(());
        }

        // Drain all pending events to avoid lag from queued scroll events.
        // Each event is collected first, then processed in order.
        let mut events = Vec::new();
        events.push(event::read()?);
        while event::poll(Duration::from_millis(0))? {
            events.push(event::read()?);
        }

        for ev in events {
            match ev {
                Event::Key(key) => {
                    if matches!(self.mode, AppMode::Startup { .. }) {
                        self.handle_startup_key(key.code)?;
                        return Ok(());
                    }

                    // Help overlay intercepts all keys.
                    if self.show_help {
                        match key.code {
                            KeyCode::Char('?') | KeyCode::Esc => {
                                self.show_help = false;
                            }
                            _ => {}
                        }
                        return Ok(());
                    }

                    // Confirm dialog intercepts all keys.
                    if self.confirm_action.is_some() {
                        self.handle_confirm_key(key.code)?;
                        return Ok(());
                    }

                    // Upgrading: only allow quit.
                    if self.upgrading {
                        match key.code {
                            KeyCode::Char('q') => {
                                self.detach_on_quit = true;
                                self.should_quit = true;
                            }
                            KeyCode::Char('Q') => {
                                self.detach_on_quit = false;
                                self.should_quit = true;
                            }
                            _ => {}
                        }
                        return Ok(());
                    }

                    // Rename mode intercepts all keys.
                    if self.rename_buf.is_some() {
                        self.handle_rename_key(key.code)?;
                        return Ok(());
                    }

                    // History mode.
                    if self.show_history && self.focus == Focus::Sidebar {
                        self.handle_history_key(key.code, key.modifiers)?;
                        return Ok(());
                    }

                    // Project view modes.
                    if self.focus == Focus::Sidebar
                        && matches!(
                            self.sidebar_view,
                            SidebarView::Projects | SidebarView::ProjectSessions(_)
                        )
                    {
                        self.handle_project_view_key(key.code, key.modifiers)?;
                        return Ok(());
                    }

                    // Filter mode: intercept char/backspace/esc, pass through navigation.
                    if self.filter_input.is_some() && self.focus == Focus::Sidebar {
                        match key.code {
                            KeyCode::Char(c) => {
                                if let Some(ref mut filter) = self.filter_input {
                                    filter.push(c);
                                }
                                self.ensure_selected_visible();
                                return Ok(());
                            }
                            KeyCode::Backspace => {
                                let should_close = self
                                    .filter_input
                                    .as_ref()
                                    .is_some_and(|f| f.is_empty());
                                if should_close {
                                    self.filter_input = None;
                                } else if let Some(ref mut filter) = self.filter_input {
                                    filter.pop();
                                }
                                self.ensure_selected_visible();
                                return Ok(());
                            }
                            KeyCode::Esc => {
                                self.filter_input = None;
                                return Ok(());
                            }
                            // Navigation and other keys fall through to normal sidebar handler.
                            _ => {}
                        }
                    }

                    match self.focus {
                        Focus::Sidebar => self.handle_sidebar_key(key.code, key.modifiers)?,
                        Focus::Terminal => self.handle_terminal_key(key.code, key.modifiers)?,
                        Focus::ProjectPicker => self.handle_picker_key(key.code)?,
                    }
                }
                Event::Paste(text) => {
                    if self.focus == Focus::Terminal {
                        if let Some(idx) = self.selected {
                            if let Some(session) = self.manager.get(idx) {
                                self.manager
                                    .tmux()
                                    .send_key_literal(&session.name, &text)?;
                            }
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    self.handle_mouse(mouse)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Check whether a mouse event is within the sidebar content area (inside borders).
    fn is_in_sidebar(&self, x: u16, y: u16) -> bool {
        x >= self.sidebar_area.x + 1
            && x < self.sidebar_area.x + self.sidebar_area.width - 1
            && y >= self.sidebar_area.y + 1
            && y < self.sidebar_area.y + self.sidebar_area.height - 1
    }

    /// Check whether a mouse event is within the terminal content area (inside borders).
    fn is_in_terminal(&self, x: u16, y: u16) -> bool {
        x >= self.terminal_area.x + 1
            && x < self.terminal_area.x + self.terminal_area.width - 1
            && y >= self.terminal_area.y + 1
            && y < self.terminal_area.y + self.terminal_area.height - 1
    }

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        if matches!(self.mode, AppMode::Startup { .. }) {
            return Ok(());
        }
        let x = mouse.column;
        let y = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.is_in_sidebar(x, y) {
                    let content_y = (y - self.sidebar_area.y - 1) as usize;

                    if self.show_history {
                        // History items are 2 lines each; account for scroll offset.
                        let offset = self.history_list_state.offset();
                        let idx = self.item_index_at_y(content_y, offset, 2);
                        if idx < self.history_entries.len() {
                            self.history_selected = idx;
                        }
                    } else {
                        match &self.sidebar_view {
                            SidebarView::Projects => {
                                // Project items are 3 lines each (name, badges, separator).
                                let offset = self.project_list_state.offset();
                                let idx = self.item_index_at_y(content_y, offset, 3);
                                let count = self.build_project_summaries().len();
                                if idx < count {
                                    self.project_selected = idx;
                                }
                            }
                            SidebarView::ProjectSessions(cwd) => {
                                let cwd = cwd.clone();
                                let offset = self.sidebar_list_state.offset();
                                let has_filter = self.filter_input.is_some();
                                let visible = self.project_session_indices(&cwd);
                                let filter_items = if has_filter { 2 } else { 0 };
                                let total_items = filter_items + visible.len();
                                let mut y_accum = 0usize;

                                for item_idx in offset..total_items {
                                    let height = if has_filter && item_idx < 2 {
                                        1
                                    } else {
                                        let vis_idx = if has_filter { item_idx - 2 } else { item_idx };
                                        if vis_idx < visible.len() {
                                            let session = &self.manager.sessions()[visible[vis_idx]];
                                            let has_prompt = session.task_label.is_some()
                                                || session.last_prompt.is_some();
                                            let has_reply = session.last_reply.as_ref()
                                                .is_some_and(|r| !r.trim().is_empty());
                                            let has_branch = session.branch.is_some();
                                            2 + has_prompt as usize + has_reply as usize + has_branch as usize
                                        } else {
                                            2
                                        }
                                    };
                                    if content_y < y_accum + height {
                                        if has_filter && item_idx < 2 {
                                            return Ok(());
                                        }
                                        let vis_idx = if has_filter { item_idx - 2 } else { item_idx };
                                        if vis_idx < visible.len() {
                                            self.selected = Some(visible[vis_idx]);
                                            self.terminal_scroll = 0;
                                            self.focus = Focus::Sidebar;
                                        }
                                        return Ok(());
                                    }
                                    y_accum += height;
                                }
                            }
                            SidebarView::Sessions => {
                                let offset = self.sidebar_list_state.offset();
                                let has_filter = self.filter_input.is_some();
                                let visible = self.visible_indices();
                                let filter_items = if has_filter { 2 } else { 0 };
                                let total_items = filter_items + visible.len();
                                let mut y_accum = 0usize;

                                for item_idx in offset..total_items {
                                    let height = if has_filter && item_idx < 2 {
                                        1
                                    } else {
                                        let vis_idx = if has_filter { item_idx - 2 } else { item_idx };
                                        if vis_idx < visible.len() {
                                            let session = &self.manager.sessions()[visible[vis_idx]];
                                            let has_prompt = session.task_label.is_some()
                                                || session.last_prompt.is_some();
                                            let has_reply = session.last_reply.as_ref()
                                                .is_some_and(|r| !r.trim().is_empty());
                                            let has_branch = session.branch.is_some();
                                            2 + has_prompt as usize + has_reply as usize + has_branch as usize
                                        } else {
                                            2
                                        }
                                    };
                                    if content_y < y_accum + height {
                                        if has_filter && item_idx < 2 {
                                            return Ok(());
                                        }
                                        let vis_idx = if has_filter { item_idx - 2 } else { item_idx };
                                        if vis_idx < visible.len() {
                                            self.selected = Some(visible[vis_idx]);
                                            self.terminal_scroll = 0;
                                            self.focus = Focus::Sidebar;
                                        }
                                        return Ok(());
                                    }
                                    y_accum += height;
                                }
                            }
                        }
                    }
                    // Clicked anywhere in sidebar (including empty space) → focus sidebar.
                    self.focus = Focus::Sidebar;
                } else if self.is_in_terminal(x, y) {
                    // Click on terminal area → focus terminal + start selection.
                    if self.selected.is_some() {
                        self.mark_selected_as_read();
                        self.focus = Focus::Terminal;
                    }
                    let content_col = x.saturating_sub(self.terminal_area.x + 1);
                    let content_row = y.saturating_sub(self.terminal_area.y + 1);
                    self.selection = Some(Selection {
                        start: (content_row, content_col),
                        end: (content_row, content_col),
                    });
                } else {
                    self.selection = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(ref mut sel) = self.selection {
                    // Clamp to terminal content area.
                    let inner_x = self.terminal_area.x + 1;
                    let inner_y = self.terminal_area.y + 1;
                    let max_col = self.terminal_area.width.saturating_sub(3);
                    let max_row = self.terminal_area.height.saturating_sub(3);
                    let content_col = x.saturating_sub(inner_x).min(max_col);
                    let content_row = y.saturating_sub(inner_y).min(max_row);
                    sel.end = (content_row, content_col);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(sel) = self.selection {
                    // If it was just a click (not a drag), clear selection.
                    if sel.start == sel.end {
                        self.selection = None;
                    }
                    // Selection stays active; user copies explicitly with 'y'.
                }
            }
            MouseEventKind::ScrollDown => {
                if self.is_in_sidebar(x, y) {
                    if self.show_history {
                        if !self.history_entries.is_empty() {
                            self.history_selected =
                                (self.history_selected + 1).min(self.history_entries.len() - 1);
                        }
                    } else {
                        match &self.sidebar_view {
                            SidebarView::Projects => {
                                let count = self.build_project_summaries().len();
                                if count > 0 {
                                    self.project_selected =
                                        (self.project_selected + 1).min(count - 1);
                                }
                            }
                            SidebarView::ProjectSessions(cwd) => {
                                let cwd = cwd.clone();
                                let visible = self.project_session_indices(&cwd);
                                if !visible.is_empty() {
                                    self.selected = Some(match self.selected {
                                        Some(current) => {
                                            if let Some(pos) =
                                                visible.iter().position(|&i| i == current)
                                            {
                                                visible[(pos + 1).min(visible.len() - 1)]
                                            } else {
                                                visible[0]
                                            }
                                        }
                                        None => visible[0],
                                    });
                                    self.terminal_scroll = 0;
                                }
                            }
                            SidebarView::Sessions => {
                                self.select_next();
                            }
                        }
                    }
                } else if self.is_in_terminal(x, y) {
                    self.terminal_scroll = self.terminal_scroll.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollUp => {
                if self.is_in_sidebar(x, y) {
                    if self.show_history {
                        self.history_selected = self.history_selected.saturating_sub(1);
                    } else {
                        match &self.sidebar_view {
                            SidebarView::Projects => {
                                self.project_selected =
                                    self.project_selected.saturating_sub(1);
                            }
                            SidebarView::ProjectSessions(cwd) => {
                                let cwd = cwd.clone();
                                let visible = self.project_session_indices(&cwd);
                                if !visible.is_empty() {
                                    self.selected = Some(match self.selected {
                                        Some(current) => {
                                            if let Some(pos) =
                                                visible.iter().position(|&i| i == current)
                                            {
                                                visible[pos.saturating_sub(1)]
                                            } else {
                                                visible[0]
                                            }
                                        }
                                        None => visible[0],
                                    });
                                    self.terminal_scroll = 0;
                                }
                            }
                            SidebarView::Sessions => {
                                self.select_prev();
                            }
                        }
                    }
                } else if self.is_in_terminal(x, y) {
                    if let Some(idx) = self.selected {
                        // Cache history size when scrolling begins to keep scrollbar stable.
                        if self.terminal_scroll == 0 {
                            self.terminal_scroll_history = self.manager.history_size(idx);
                        }
                        self.terminal_scroll = self.terminal_scroll
                            .saturating_add(3)
                            .min(self.terminal_scroll_history);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Given a content_y position, scroll offset, and fixed item height,
    /// return the item index (for uniform-height lists like history).
    fn item_index_at_y(&self, content_y: usize, offset: usize, item_height: usize) -> usize {
        offset + content_y / item_height
    }

    fn handle_startup_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Char('r') => {
                // Restore previous sessions.
                self.manager.restore();
                if !self.manager.is_empty() {
                    self.selected = Some(0);
                }
                self.mode = AppMode::Normal;
            }
            KeyCode::Char('n') => {
                // Start fresh: kill all existing chatmux sessions.
                self.manager.kill_all_chatmux_sessions();
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

    fn handle_sidebar_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        match code {
            KeyCode::Char('q') => {
                // Detach: keep tmux sessions alive, save state.
                self.detach_on_quit = true;
                self.should_quit = true;
            }
            KeyCode::Char('Q') => {
                // Full quit: kill all sessions.
                self.detach_on_quit = false;
                self.should_quit = true;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl-C: detach (same as q).
                self.detach_on_quit = true;
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Enter => {
                if self.selected.is_some() {
                    self.filter_input = None;
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
                // Open selected session's project directory in $EDITOR.
                if let Some(idx) = self.selected {
                    if let Some(session) = self.manager.get(idx) {
                        self.open_editor(&session.cwd.clone())?;
                    }
                }
            }
            KeyCode::Char('d') => {
                if let Some(idx) = self.selected {
                    self.manager.remove(idx)?;
                    if self.manager.is_empty() {
                        self.selected = None;
                        self.terminal_content.clear();
                    } else {
                        self.selected = Some(idx.min(self.manager.len() - 1));
                    }
                }
            }
            KeyCode::Char('r') => {
                // Start rename mode.
                if let Some(idx) = self.selected {
                    if let Some(session) = self.manager.get(idx) {
                        let current = session.task_label.clone().unwrap_or_default();
                        self.rename_buf = Some(current);
                    }
                }
            }
            KeyCode::Char('s') => {
                // Cycle sort mode.
                self.sort_mode = self.sort_mode.next();
                self.auto_sort();
            }
            KeyCode::Char('/') => {
                // Start filter mode.
                self.filter_input = Some(String::new());
            }
            KeyCode::Char('h') => {
                // Enter history mode.
                self.show_history = true;
                self.history_entries = crate::session::state::load_history();
                self.history_selected = 0;
            }
            KeyCode::Char('p') => {
                // Toggle project view.
                self.sidebar_view = SidebarView::Projects;
                self.project_selected = 0;
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
    fn handle_project_view_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        match &self.sidebar_view {
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
                            self.project_selected =
                                (self.project_selected + 1).min(count - 1);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.project_selected = self.project_selected.saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        // Drill into the selected project's sessions.
                        let summaries = self.build_project_summaries();
                        if let Some(proj) = summaries.get(self.project_selected) {
                            let cwd = proj.cwd.clone();
                            let indices = self.project_session_indices(&cwd);
                            self.sidebar_view = SidebarView::ProjectSessions(cwd);
                            // Select the first session in this project.
                            self.selected = indices.first().copied();
                            self.terminal_scroll = 0;
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('p') => {
                        // Return to flat session view.
                        self.sidebar_view = SidebarView::Sessions;
                    }
                    KeyCode::Char('n') => {
                        // Allow creating new sessions from project view too.
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
                        // Go back to project list.
                        self.sidebar_view = SidebarView::Projects;
                    }
                    KeyCode::Char('p') => {
                        // Back to flat session view.
                        self.sidebar_view = SidebarView::Sessions;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        let visible = self.project_session_indices(&cwd_owned);
                        if visible.is_empty() {
                            return Ok(());
                        }
                        self.selected = Some(match self.selected {
                            Some(current) => {
                                if let Some(pos) =
                                    visible.iter().position(|&i| i == current)
                                {
                                    visible[(pos + 1).min(visible.len() - 1)]
                                } else {
                                    visible[0]
                                }
                            }
                            None => visible[0],
                        });
                        self.terminal_scroll = 0;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let visible = self.project_session_indices(&cwd_owned);
                        if visible.is_empty() {
                            return Ok(());
                        }
                        self.selected = Some(match self.selected {
                            Some(current) => {
                                if let Some(pos) =
                                    visible.iter().position(|&i| i == current)
                                {
                                    visible[pos.saturating_sub(1)]
                                } else {
                                    visible[0]
                                }
                            }
                            None => visible[0],
                        });
                        self.terminal_scroll = 0;
                    }
                    KeyCode::Enter => {
                        if self.selected.is_some() {
                            self.mark_selected_as_read();
                            self.focus = Focus::Terminal;
                        }
                    }
                    KeyCode::Char('d') => {
                        if let Some(idx) = self.selected {
                            self.manager.remove(idx)?;
                            let visible = self.project_session_indices(&cwd_owned);
                            if visible.is_empty() {
                                // No more sessions in this project, go back.
                                self.selected = None;
                                self.terminal_content.clear();
                                self.sidebar_view = SidebarView::Projects;
                            } else {
                                self.selected = Some(visible[0]);
                            }
                        }
                    }
                    KeyCode::Char('r') => {
                        // Start rename mode.
                        if let Some(idx) = self.selected {
                            if let Some(session) = self.manager.get(idx) {
                                let current =
                                    session.task_label.clone().unwrap_or_default();
                                self.rename_buf = Some(current);
                            }
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
                        if let Some(idx) = self.selected {
                            if let Some(session) = self.manager.get(idx) {
                                self.open_editor(&session.cwd.clone())?;
                            }
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
            SidebarView::Sessions => {
                // Should not reach here, but handle gracefully.
            }
        }
        Ok(())
    }

    fn handle_confirm_key(&mut self, code: KeyCode) -> Result<()> {
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

    fn handle_rename_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Char(c) => {
                if let Some(ref mut buf) = self.rename_buf {
                    buf.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut buf) = self.rename_buf {
                    buf.pop();
                }
            }
            KeyCode::Enter => {
                // Commit rename.
                if let (Some(idx), Some(buf)) = (self.selected, self.rename_buf.take()) {
                    if let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                        session.task_label = if buf.is_empty() { None } else { Some(buf) };
                    }
                }
            }
            KeyCode::Esc => {
                self.rename_buf = None;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_history_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        match code {
            KeyCode::Char('h') | KeyCode::Esc => {
                self.show_history = false;
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
                if !self.history_entries.is_empty() {
                    self.history_selected =
                        (self.history_selected + 1).min(self.history_entries.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.history_selected = self.history_selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                // Restart session from history entry.
                if let Some(entry) = self.history_entries.get(self.history_selected) {
                    let cwd = entry.cwd.clone();
                    let agent_kind = entry.agent_kind;
                    self.show_history = false;
                    self.create_session(&cwd, agent_kind)?;
                }
            }
            KeyCode::Char('d') => {
                // Delete history entry.
                if self.history_selected < self.history_entries.len() {
                    self.history_entries.remove(self.history_selected);
                    crate::session::state::save_history(&self.history_entries);
                    if !self.history_entries.is_empty() {
                        self.history_selected =
                            self.history_selected.min(self.history_entries.len() - 1);
                    }
                }
            }
            KeyCode::Char('?') => {
                self.show_help = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_terminal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        // When selection is active, handle copy keys before forwarding to tmux.
        if let Some(sel) = self.selection {
            if sel.start != sel.end {
                let is_copy = matches!(code, KeyCode::Char('y'))
                    || (matches!(code, KeyCode::Char('c'))
                        && (modifiers.contains(KeyModifiers::CONTROL)
                            || modifiers.contains(KeyModifiers::SUPER)));
                if is_copy {
                    self.copy_selection_to_clipboard(&sel);
                    self.selection = None;
                    return Ok(());
                }
                // Esc clears selection without copying.
                if matches!(code, KeyCode::Esc) {
                    self.selection = None;
                    return Ok(());
                }
            }
        }

        // Any key input in terminal clears selection and resets scroll.
        self.selection = None;
        self.terminal_scroll = 0;

        // Prefix mode: Ctrl+] was pressed, interpret next key as a command.
        if self.prefix_active {
            self.prefix_active = false;
            match code {
                // Prefix + Esc or [ → switch to sidebar
                KeyCode::Esc | KeyCode::Char('[') => {
                    self.focus = Focus::Sidebar;
                    return Ok(());
                }
                // Prefix + Ctrl+] → send literal Ctrl+] to tmux
                _ if is_prefix_key(code, modifiers) => {
                    if let Some(idx) = self.selected {
                        self.manager.send_keys(idx, "C-]")?;
                    }
                    return Ok(());
                }
                // Prefix + ? → show help overlay
                KeyCode::Char('?') => {
                    self.show_help = true;
                    return Ok(());
                }
                // Any other key → ignore (prefix cancelled)
                _ => return Ok(()),
            }
        }

        // Ctrl+] → enter prefix mode
        if is_prefix_key(code, modifiers) {
            self.prefix_active = true;
            return Ok(());
        }

        let Some(idx) = self.selected else {
            return Ok(());
        };

        // Build tmux key name from crossterm KeyCode + modifiers.
        // We translate every key into the tmux `send-keys` name so that
        // all keyboard events are transparently forwarded to the child process.
        let base_name: Option<String> = match code {
            KeyCode::Char(c) => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    Some(format!("C-{c}"))
                } else if modifiers.contains(KeyModifiers::ALT) {
                    Some(format!("M-{c}"))
                } else {
                    // Plain character: send as literal to preserve special chars.
                    self.manager
                        .tmux()
                        .send_key_literal(
                            &self.manager.get(idx).unwrap().name,
                            &c.to_string(),
                        )?;
                    None // already sent
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
            _ => None,
        };

        if let Some(mut key) = base_name {
            // Apply Shift/Alt/Ctrl prefixes for non-Char keys.
            // (Char keys already handle Ctrl/Alt above.)
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
            self.manager.send_keys(idx, &key)?;
        }
        Ok(())
    }

    fn handle_picker_key(&mut self, code: KeyCode) -> Result<()> {
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
                if !picker.on_backspace_filter() {
                    // Filter was empty — fall through to existing behavior.
                    if picker.mode == PickerMode::DirectoryBrowser {
                        picker.go_up();
                    }
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
