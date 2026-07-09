use super::*;

impl App {
    pub fn handle_event(&mut self) -> Result<()> {
        if !event::poll(Duration::from_millis(100))? {
            return Ok(());
        }

        // Drain all pending events to avoid lag from queued scroll events.
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
                    if self.sidebar.rename_buf.is_some() {
                        self.handle_rename_key(key.code)?;
                        return Ok(());
                    }

                    // History mode.
                    if self.sidebar.show_history && self.focus == Focus::Sidebar {
                        self.handle_history_key(key.code, key.modifiers)?;
                        return Ok(());
                    }

                    // Project view modes.
                    if self.focus == Focus::Sidebar
                        && matches!(
                            self.sidebar.view,
                            SidebarView::Projects | SidebarView::ProjectSessions(_)
                        )
                    {
                        self.handle_project_view_key(key.code, key.modifiers)?;
                        return Ok(());
                    }

                    // Filter mode: intercept char/backspace/esc, pass through navigation.
                    if self.sidebar.filter_input.is_some() && self.focus == Focus::Sidebar {
                        match key.code {
                            KeyCode::Char(c) => {
                                if let Some(ref mut filter) = self.sidebar.filter_input {
                                    filter.push(c);
                                }
                                self.ensure_selected_visible();
                                return Ok(());
                            }
                            KeyCode::Backspace => {
                                let should_close = self
                                    .sidebar.filter_input
                                    .as_ref()
                                    .is_some_and(|f| f.is_empty());
                                if should_close {
                                    self.sidebar.filter_input = None;
                                } else if let Some(ref mut filter) = self.sidebar.filter_input {
                                    filter.pop();
                                }
                                self.ensure_selected_visible();
                                return Ok(());
                            }
                            KeyCode::Esc => {
                                self.sidebar.filter_input = None;
                                return Ok(());
                            }
                            _ => {}
                        }
                    }

                    match self.focus {
                        Focus::Sidebar => self.handle_sidebar_key(key.code, key.modifiers)?,
                        Focus::Terminal => self.handle_terminal_key(key.code, key.modifiers)?,
                        Focus::ProjectPicker => self.handle_picker_key(key.code, key.modifiers)?,
                    }
                }
                Event::Paste(text) => {
                    if self.focus == Focus::Terminal
                        && let Some(idx) = self.selected_index()
                            && let Some(session) = self.manager.get(idx) {
                                let _ = self.manager
                                    .tmux()
                                    .paste_text(&session.name, &text);
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
}
