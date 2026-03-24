use crate::session::SessionManager;
use crate::tui::{render_sidebar, render_terminal};
use crate::tui::sidebar::render_summary_bar;
use crate::tui::terminal::render_empty_terminal;
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Terminal,
    ProjectInput,
}

pub struct App {
    manager: SessionManager,
    selected: Option<usize>,
    focus: Focus,
    should_quit: bool,
    terminal_content: String,
    project_input: String,
}

impl App {
    pub fn new() -> Self {
        Self {
            manager: SessionManager::new(),
            selected: None,
            focus: Focus::Sidebar,
            should_quit: false,
            terminal_content: String::new(),
            project_input: String::new(),
        }
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn tick(&mut self) {
        // Refresh terminal content for the selected session.
        if let Some(idx) = self.selected {
            if let Ok(content) = self.manager.capture(idx) {
                self.terminal_content = content;
            }
        }
    }

    pub fn draw(&self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(35), Constraint::Min(1)])
            .split(frame.area());

        // Sidebar area: split into list + summary bar.
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

        // Terminal area or project input.
        match self.focus {
            Focus::ProjectInput => {
                self.draw_project_input(frame, chunks[1]);
            }
            _ => {
                if self.selected.is_some() {
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
        }
    }

    fn draw_project_input(&self, frame: &mut Frame, area: Rect) {
        use ratatui::style::{Color, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Paragraph};

        let block = Block::default()
            .title(" New Session — Enter project path ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));

        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("  Path: "),
                Span::styled(
                    format!("{}_", &self.project_input),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  Enter: confirm  Esc: cancel  Tab: expand ~",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(lines).block(block);
        frame.render_widget(paragraph, area);
    }

    pub fn handle_event(&mut self) -> Result<()> {
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match self.focus {
                    Focus::Sidebar => self.handle_sidebar_key(key.code, key.modifiers)?,
                    Focus::Terminal => self.handle_terminal_key(key.code, key.modifiers)?,
                    Focus::ProjectInput => self.handle_project_input_key(key.code)?,
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
                self.project_input.clear();
                self.focus = Focus::ProjectInput;
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
        // Esc returns to sidebar.
        if code == KeyCode::Esc {
            self.focus = Focus::Sidebar;
            return Ok(());
        }

        let Some(idx) = self.selected else {
            return Ok(());
        };

        // Forward keys to tmux.
        match code {
            KeyCode::Char(c) => {
                if modifiers.contains(KeyModifiers::CONTROL) {
                    let ctrl_key = format!("C-{c}");
                    self.manager.send_keys(idx, &ctrl_key)?;
                } else {
                    self.manager
                        .tmux()
                        .send_key_literal(&self.manager.get(idx).unwrap().name, &c.to_string())?;
                }
            }
            KeyCode::Enter => {
                self.manager.send_keys(idx, "Enter")?;
            }
            KeyCode::Backspace => {
                self.manager.send_keys(idx, "BSpace")?;
            }
            KeyCode::Tab => {
                self.manager.send_keys(idx, "Tab")?;
            }
            KeyCode::Up => {
                self.manager.send_keys(idx, "Up")?;
            }
            KeyCode::Down => {
                self.manager.send_keys(idx, "Down")?;
            }
            KeyCode::Left => {
                self.manager.send_keys(idx, "Left")?;
            }
            KeyCode::Right => {
                self.manager.send_keys(idx, "Right")?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_project_input_key(&mut self, code: KeyCode) -> Result<()> {
        match code {
            KeyCode::Esc => {
                self.focus = Focus::Sidebar;
            }
            KeyCode::Enter => {
                let path = self.resolve_path(&self.project_input.clone());
                if std::path::Path::new(&path).is_dir() {
                    let idx = self.manager.create(&path)?;
                    self.selected = Some(idx);
                    self.focus = Focus::Sidebar;
                }
                // TODO: show error if path is invalid
            }
            KeyCode::Tab => {
                // Expand ~ to home directory.
                if self.project_input.starts_with('~') {
                    if let Some(home) = dirs_home() {
                        self.project_input =
                            self.project_input.replacen('~', &home, 1);
                    }
                }
            }
            KeyCode::Char(c) => {
                self.project_input.push(c);
            }
            KeyCode::Backspace => {
                self.project_input.pop();
            }
            _ => {}
        }
        Ok(())
    }

    fn resolve_path(&self, input: &str) -> String {
        if input.starts_with('~') {
            if let Some(home) = dirs_home() {
                return input.replacen('~', &home, 1);
            }
        }
        input.to_string()
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

    /// Resize the selected session's tmux pane to match the terminal view.
    pub fn resize_selected_pane(&self, terminal_area: Rect) {
        if let Some(idx) = self.selected {
            // Subtract 2 for the border.
            let width = terminal_area.width.saturating_sub(2);
            let height = terminal_area.height.saturating_sub(2);
            if width > 0 && height > 0 {
                let _ = self.manager.resize(idx, width, height);
            }
        }
    }
}

fn dirs_home() -> Option<String> {
    std::env::var("HOME").ok()
}
