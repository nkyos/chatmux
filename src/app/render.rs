use super::*;

impl App {
    pub fn draw(&mut self, frame: &mut Frame) {
        // Startup screen: full-screen restore prompt.
        if let AppMode::Startup {
            ref existing_sessions,
        } = self.mode
        {
            render_startup_screen(frame, frame.area(), existing_sessions);
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(self.config.display.sidebar_width),
                Constraint::Min(1),
            ])
            .split(frame.area());

        // Sidebar: list + hint + summary bar.
        let sidebar_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1), Constraint::Length(1)])
            .split(chunks[0]);

        if self.show_history {
            render_history_sidebar(
                frame,
                sidebar_chunks[0],
                &self.history_entries,
                self.history_selected,
                self.focus == Focus::Sidebar,
                &self.theme,
                &mut self.history_list_state,
            );
        } else {
            match &self.sidebar_view {
                SidebarView::Projects => {
                    let summaries = self.build_project_summaries();
                    render_project_list(
                        frame,
                        sidebar_chunks[0],
                        &summaries,
                        self.project_selected,
                        self.focus == Focus::Sidebar,
                        &self.theme,
                        &mut self.project_list_state,
                    );
                }
                SidebarView::ProjectSessions(cwd) => {
                    let visible = self.project_session_indices(cwd);
                    let project_name = self
                        .manager
                        .sessions()
                        .iter()
                        .find(|s| s.cwd == *cwd)
                        .map(|s| s.project_name.clone())
                        .unwrap_or_default();
                    let title = format!(" {} [{}] ", project_name, visible.len());
                    render_sidebar_with_title(
                        frame,
                        sidebar_chunks[0],
                        self.manager.sessions(),
                        self.selected,
                        self.focus == Focus::Sidebar,
                        &self.theme,
                        self.sort_mode,
                        self.filter_input.as_deref(),
                        self.rename_buf
                            .as_ref()
                            .map(|buf| (self.selected.unwrap_or(0), buf.as_str())),
                        &visible,
                        &mut self.sidebar_list_state,
                        Some(&title),
                    );
                }
                SidebarView::Sessions => {
                    let visible = self.visible_indices();
                    render_sidebar(
                        frame,
                        sidebar_chunks[0],
                        self.manager.sessions(),
                        self.selected,
                        self.focus == Focus::Sidebar,
                        &self.theme,
                        self.sort_mode,
                        self.filter_input.as_deref(),
                        self.rename_buf
                            .as_ref()
                            .map(|buf| (self.selected.unwrap_or(0), buf.as_str())),
                        &visible,
                        &mut self.sidebar_list_state,
                    );
                }
            }
        }
        render_summary_bar(
            frame,
            sidebar_chunks[1],
            self.manager.sessions(),
            &self.theme,
        );

        // Prefix key hint at the bottom of the sidebar.
        {
            use ratatui::text::{Line, Span};
            use ratatui::style::{Color, Style};
            let hint = Line::from(vec![
                Span::styled(" C-] ", Style::default().fg(Color::Cyan)),
                Span::styled("prefix  ", Style::default().fg(Color::DarkGray)),
                Span::styled("? ", Style::default().fg(Color::Cyan)),
                Span::styled("help", Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(hint, sidebar_chunks[2]);
        }

        // Right pane: project picker, upgrade output, or terminal.
        if self.upgrading {
            render_terminal(
                frame,
                chunks[1],
                &self.terminal_content,
                Some("Upgrading..."),
                true,
                &self.theme,
                None,
            );
        } else if let Some(ref picker) = self.picker {
            render_project_picker(frame, chunks[1], picker);
        } else if self.selected.is_some() {
            let label = self
                .selected
                .and_then(|i| self.manager.get(i))
                .map(|s| {
                    let base = s.display_label().to_string();
                    let mut label = if self.terminal_scroll > 0 {
                        format!("{base} [scroll: -{}]", self.terminal_scroll)
                    } else {
                        base
                    };
                    if self.prefix_active {
                        label.push_str(" [C-] ...]");
                    }
                    label
                });
            let scroll_info = if self.terminal_scroll > 0 {
                Some(TerminalScroll {
                    offset: self.terminal_scroll,
                    history_size: self.terminal_scroll_history,
                })
            } else {
                None
            };
            render_terminal(
                frame,
                chunks[1],
                &self.terminal_content,
                label.as_deref(),
                self.focus == Focus::Terminal,
                &self.theme,
                scroll_info.as_ref(),
            );

            // Render selection highlight by reversing cell styles.
            if let Some(ref sel) = self.selection {
                let ((r1, c1), (r2, c2)) = sel.ordered();
                let inner_x = chunks[1].x + 1;
                let inner_y = chunks[1].y + 1;
                let inner_w = chunks[1].width.saturating_sub(2);
                let inner_h = chunks[1].height.saturating_sub(2);
                let buf = frame.buffer_mut();
                for row in r1..=r2 {
                    if row >= inner_h {
                        break;
                    }
                    let col_start = if row == r1 { c1 } else { 0 };
                    let col_end = if row == r2 { c2 + 1 } else { inner_w };
                    for col in col_start..col_end.min(inner_w) {
                        let cell =
                            &mut buf[(inner_x + col, inner_y + row)];
                        // Swap fg/bg for selection highlight.
                        let fg = cell.fg;
                        let bg = cell.bg;
                        cell.set_fg(if bg == ratatui::style::Color::Reset {
                            ratatui::style::Color::Black
                        } else {
                            bg
                        });
                        cell.set_bg(if fg == ratatui::style::Color::Reset {
                            ratatui::style::Color::White
                        } else {
                            fg
                        });
                    }
                }
            }
        } else {
            render_empty_terminal(frame, chunks[1], &self.theme);
        }

        // Confirm overlay (drawn on top).
        if let Some(ref action) = self.confirm_action {
            let n = self.session_count();
            let msg = match action {
                ConfirmAction::UpgradeAndRestart => {
                    if n == 0 {
                        "Upgrade agents?".to_string()
                    } else {
                        format!("Upgrade and restart {} session{}?", n, if n == 1 { "" } else { "s" })
                    }
                }
                ConfirmAction::RestartAll => {
                    format!("Restart all {} session{}?", n, if n == 1 { "" } else { "s" })
                }
            };
            render_confirm_overlay(frame, frame.area(), &msg);
        }

        // Help overlay (drawn last, on top of everything).
        if self.show_help {
            let ctx = self.current_help_context();
            render_help_overlay(frame, frame.area(), ctx);
        }
    }

    /// Determine which help context to show based on current view state.
    fn current_help_context(&self) -> HelpContext {
        if self.show_history {
            return HelpContext::History;
        }
        if self.focus == Focus::Terminal {
            return HelpContext::Terminal;
        }
        match &self.sidebar_view {
            SidebarView::Projects => HelpContext::Projects,
            SidebarView::ProjectSessions(_) => HelpContext::ProjectSessions,
            SidebarView::Sessions => HelpContext::Sessions,
        }
    }

    /// Update the cached layout areas based on the current terminal size.
    pub fn update_layout(&mut self, size: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(self.config.display.sidebar_width),
                Constraint::Min(1),
            ])
            .split(size);
        self.sidebar_area = chunks[0];
        self.terminal_area = chunks[1];
    }
}
