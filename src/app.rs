use crate::session::SessionManager;
use crate::tui::project_picker::{PickerMode, ProjectPicker, render_project_picker};
use crate::tui::sidebar::render_summary_bar;
use crate::tui::terminal::render_empty_terminal;
use crate::tui::{render_sidebar, render_terminal};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};
use std::time::Duration;

const SIDEBAR_WIDTH: u16 = 35;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Terminal,
    ProjectPicker,
}

pub struct App {
    manager: SessionManager,
    selected: Option<usize>,
    focus: Focus,
    should_quit: bool,
    terminal_content: String,
    picker: Option<ProjectPicker>,
    /// Cached terminal area for pane sizing.
    terminal_area: Rect,
}

impl App {
    pub fn new() -> Self {
        Self {
            manager: SessionManager::new(),
            selected: None,
            focus: Focus::Sidebar,
            should_quit: false,
            terminal_content: String::new(),
            picker: None,
            terminal_area: Rect::default(),
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Called every frame before draw. Resizes tmux panes and captures content.
    pub fn tick(&mut self) {
        // Resize all sessions' tmux panes to match the terminal view area.
        let pane_width = self.terminal_area.width.saturating_sub(2);
        let pane_height = self.terminal_area.height.saturating_sub(2);
        if pane_width > 0 && pane_height > 0 {
            for i in 0..self.manager.len() {
                let _ = self.manager.resize(i, pane_width, pane_height);
            }
        }

        // Capture content for the selected session.
        if let Some(idx) = self.selected {
            if let Ok(content) = self.manager.capture(idx) {
                self.terminal_content = content;
            }
        }
    }

    /// Update the cached terminal area based on the current terminal size.
    pub fn update_layout(&mut self, size: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)])
            .split(size);
        self.terminal_area = chunks[1];
    }

    pub fn draw(&self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)])
            .split(frame.area());

        // Sidebar: list + summary bar.
        let sidebar_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(chunks[0]);

        render_sidebar(
            frame,
            sidebar_chunks[0],
            self.manager.sessions(),
            self.selected,
            self.focus == Focus::Sidebar,
        );
        render_summary_bar(frame, sidebar_chunks[1], self.manager.sessions());

        // Right pane: project picker or terminal.
        if let Some(ref picker) = self.picker {
            render_project_picker(frame, chunks[1], picker);
        } else if self.selected.is_some() {
            let label = self
                .selected
                .and_then(|i| self.manager.get(i))
                .map(|s| s.display_label().to_string());
            render_terminal(
                frame,
                chunks[1],
                &self.terminal_content,
                label.as_deref(),
                self.focus == Focus::Terminal,
            );
        } else {
            render_empty_terminal(frame, chunks[1]);
        }
    }

    pub fn handle_event(&mut self) -> Result<()> {
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match self.focus {
                    Focus::Sidebar => self.handle_sidebar_key(key.code, key.modifiers)?,
                    Focus::Terminal => self.handle_terminal_key(key.code, key.modifiers)?,
                    Focus::ProjectPicker => self.handle_picker_key(key.code)?,
                }
            }
        }
        Ok(())
    }

    fn handle_sidebar_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Enter => {
                if self.selected.is_some() {
                    self.focus = Focus::Terminal;
                }
            }
            KeyCode::Char('n') => {
                self.picker = Some(ProjectPicker::new());
                self.focus = Focus::ProjectPicker;
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
            _ => {}
        }
        Ok(())
    }

    fn handle_terminal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        if code == KeyCode::Esc {
            self.focus = Focus::Sidebar;
            return Ok(());
        }

        let Some(idx) = self.selected else {
            return Ok(());
        };

        match code {
            KeyCode::Char(c) => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    let ctrl_key = format!("C-{c}");
                    self.manager.send_keys(idx, &ctrl_key)?;
                } else {
                    self.manager
                        .tmux()
                        .send_key_literal(
                            &self.manager.get(idx).unwrap().name,
                            &c.to_string(),
                        )?;
                }
            }
            KeyCode::Enter => self.manager.send_keys(idx, "Enter")?,
            KeyCode::Backspace => self.manager.send_keys(idx, "BSpace")?,
            KeyCode::Tab => self.manager.send_keys(idx, "Tab")?,
            KeyCode::Up => self.manager.send_keys(idx, "Up")?,
            KeyCode::Down => self.manager.send_keys(idx, "Down")?,
            KeyCode::Left => self.manager.send_keys(idx, "Left")?,
            KeyCode::Right => self.manager.send_keys(idx, "Right")?,
            _ => {}
        }
        Ok(())
    }

    fn handle_picker_key(&mut self, code: KeyCode) -> Result<()> {
        let Some(ref mut picker) = self.picker else {
            return Ok(());
        };

        match code {
            KeyCode::Esc => {
                if picker.mode == PickerMode::DirectoryBrowser {
                    picker.back_to_recent();
                } else {
                    self.picker = None;
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => picker.move_down(),
            KeyCode::Char('k') | KeyCode::Up => picker.move_up(),
            KeyCode::Enter => {
                if let Some(path) = picker.confirm() {
                    self.create_session(&path)?;
                }
            }
            KeyCode::Char(' ') => {
                if let Some(path) = picker.select_current_dir() {
                    self.create_session(&path)?;
                }
            }
            KeyCode::Backspace => {
                if picker.mode == PickerMode::DirectoryBrowser {
                    picker.go_up();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn create_session(&mut self, path: &str) -> Result<()> {
        // Use the terminal area size (minus borders) for the tmux pane.
        let width = self.terminal_area.width.saturating_sub(2).max(80);
        let height = self.terminal_area.height.saturating_sub(2).max(24);
        let idx = self.manager.create(path, width, height)?;
        self.selected = Some(idx);
        self.picker = None;
        self.focus = Focus::Sidebar;
        Ok(())
    }

    fn select_next(&mut self) {
        if self.manager.is_empty() {
            return;
        }
        self.selected = Some(match self.selected {
            Some(i) => (i + 1).min(self.manager.len() - 1),
            None => 0,
        });
    }

    fn select_prev(&mut self) {
        if self.manager.is_empty() {
            return;
        }
        self.selected = Some(match self.selected {
            Some(i) => i.saturating_sub(1),
            None => 0,
        });
    }

    pub fn cleanup(&mut self) {
        self.manager.cleanup();
    }
}
