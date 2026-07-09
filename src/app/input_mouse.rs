use super::*;

impl App {
    /// Check whether a mouse event is within the sidebar content area (inside borders).
    pub(super) fn is_in_sidebar(&self, x: u16, y: u16) -> bool {
        x > self.sidebar.area.x
            && x < self.sidebar.area.x + self.sidebar.area.width - 1
            && y > self.sidebar.area.y
            && y < self.sidebar.area.y + self.sidebar.area.height - 1
    }

    /// Check whether a mouse event is within the terminal content area (inside borders).
    pub(super) fn is_in_terminal(&self, x: u16, y: u16) -> bool {
        x > self.terminal.area.x
            && x < self.terminal.area.x + self.terminal.area.width - 1
            && y > self.terminal.area.y
            && y < self.terminal.area.y + self.terminal.area.height - 1
    }

    pub(super) fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        if matches!(self.mode, AppMode::Startup { .. }) {
            return Ok(());
        }
        let x = mouse.column;
        let y = mouse.row;

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.is_in_sidebar(x, y) {
                    let content_y = (y - self.sidebar.area.y - 1) as usize;

                    if self.sidebar.show_history {
                        // History items are 2 lines each; account for scroll offset.
                        let offset = self.sidebar.history_list_state.offset();
                        let idx = self.item_index_at_y(content_y, offset, 2);
                        if idx < self.sidebar.history_entries.len() {
                            self.sidebar.history_selected = idx;
                        }
                    } else {
                        match &self.sidebar.view {
                            SidebarView::Projects => {
                                // Project items are 3 lines each (name, badges, separator).
                                let offset = self.sidebar.project_list_state.offset();
                                let idx = self.item_index_at_y(content_y, offset, 3);
                                let count = self.build_project_summaries().len();
                                if idx < count {
                                    self.sidebar.project_selected = idx;
                                }
                            }
                            SidebarView::ProjectSessions(cwd) => {
                                let cwd = cwd.clone();
                                let visible = self.project_session_indices(&cwd);
                                self.click_session_list(content_y, &visible)?;
                            }
                            SidebarView::Sessions => {
                                let visible = self.visible_indices();
                                self.click_session_list(content_y, &visible)?;
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
                    let content_col = x.saturating_sub(self.terminal.area.x + 1);
                    let content_row = y.saturating_sub(self.terminal.area.y + 1);
                    self.terminal.selection = Some(Selection {
                        start: (content_row, content_col),
                        end: (content_row, content_col),
                    });
                } else {
                    self.terminal.selection = None;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(ref mut sel) = self.terminal.selection {
                    // Clamp to terminal content area.
                    let inner_x = self.terminal.area.x + 1;
                    let inner_y = self.terminal.area.y + 1;
                    let max_col = self.terminal.area.width.saturating_sub(3);
                    let max_row = self.terminal.area.height.saturating_sub(3);
                    let content_col = x.saturating_sub(inner_x).min(max_col);
                    let content_row = y.saturating_sub(inner_y).min(max_row);
                    sel.end = (content_row, content_col);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(sel) = self.terminal.selection {
                    // If it was just a click (not a drag), clear selection.
                    if sel.start == sel.end {
                        self.terminal.selection = None;
                    } else {
                        // Auto-copy to clipboard via OSC 52 on selection complete.
                        self.copy_selection_to_clipboard(&sel);
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if self.is_in_sidebar(x, y) {
                    if self.sidebar.show_history {
                        if !self.sidebar.history_entries.is_empty() {
                            self.sidebar.history_selected =
                                (self.sidebar.history_selected + 1).min(self.sidebar.history_entries.len() - 1);
                        }
                    } else {
                        match &self.sidebar.view {
                            SidebarView::Projects => {
                                let count = self.build_project_summaries().len();
                                if count > 0 {
                                    self.sidebar.project_selected =
                                        (self.sidebar.project_selected + 1).min(count - 1);
                                }
                            }
                            SidebarView::ProjectSessions(cwd) => {
                                let cwd = cwd.clone();
                                let visible = self.project_session_indices(&cwd);
                                self.select_next_in(&visible);
                            }
                            SidebarView::Sessions => {
                                self.select_next();
                            }
                        }
                    }
                } else if self.is_in_terminal(x, y) {
                    self.terminal.scroll = self.terminal.scroll.saturating_sub(3);
                }
            }
            MouseEventKind::ScrollUp => {
                if self.is_in_sidebar(x, y) {
                    if self.sidebar.show_history {
                        self.sidebar.history_selected = self.sidebar.history_selected.saturating_sub(1);
                    } else {
                        match &self.sidebar.view {
                            SidebarView::Projects => {
                                self.sidebar.project_selected =
                                    self.sidebar.project_selected.saturating_sub(1);
                            }
                            SidebarView::ProjectSessions(cwd) => {
                                let cwd = cwd.clone();
                                let visible = self.project_session_indices(&cwd);
                                self.select_prev_in(&visible);
                            }
                            SidebarView::Sessions => {
                                self.select_prev();
                            }
                        }
                    }
                } else if self.is_in_terminal(x, y)
                    && let Some(idx) = self.selected_index() {
                        if self.terminal.scroll == 0 {
                            self.terminal.scroll_history = self.manager.history_size(idx);
                        }
                        self.terminal.scroll = self.terminal.scroll
                            .saturating_add(3)
                            .min(self.terminal.scroll_history);
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
}
