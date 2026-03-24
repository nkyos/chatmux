use crate::projects;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Padding},
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    RecentProjects,
    DirectoryBrowser,
}

pub struct ProjectPicker {
    pub mode: PickerMode,
    /// Recent projects from Claude history.
    pub recent_projects: Vec<String>,
    /// Currently selected index in the list.
    pub selected: usize,
    /// Directory browser: current directory.
    pub browser_cwd: String,
    /// Directory browser: entries in current directory.
    pub browser_entries: Vec<String>,
    /// Directory browser: selected index.
    pub browser_selected: usize,
}

impl ProjectPicker {
    pub fn new() -> Self {
        let recent_projects = projects::discover_projects();
        Self {
            mode: PickerMode::RecentProjects,
            recent_projects,
            selected: 0,
            browser_cwd: dirs_home(),
            browser_entries: Vec::new(),
            browser_selected: 0,
        }
    }

    pub fn total_items(&self) -> usize {
        // Recent projects + "Browse..." option.
        self.recent_projects.len() + 1
    }

    pub fn move_down(&mut self) {
        match self.mode {
            PickerMode::RecentProjects => {
                if self.selected < self.total_items() - 1 {
                    self.selected += 1;
                }
            }
            PickerMode::DirectoryBrowser => {
                if self.browser_selected < self.browser_entries.len().saturating_sub(1) {
                    self.browser_selected += 1;
                }
            }
        }
    }

    pub fn move_up(&mut self) {
        match self.mode {
            PickerMode::RecentProjects => {
                self.selected = self.selected.saturating_sub(1);
            }
            PickerMode::DirectoryBrowser => {
                self.browser_selected = self.browser_selected.saturating_sub(1);
            }
        }
    }

    /// Handle Enter. Returns Some(path) if a project was selected.
    pub fn confirm(&mut self) -> Option<String> {
        match self.mode {
            PickerMode::RecentProjects => {
                if self.selected < self.recent_projects.len() {
                    // Selected a recent project.
                    Some(self.recent_projects[self.selected].clone())
                } else {
                    // Selected "Browse...".
                    self.enter_browser();
                    None
                }
            }
            PickerMode::DirectoryBrowser => {
                if self.browser_entries.is_empty() {
                    return None;
                }
                let entry = &self.browser_entries[self.browser_selected];
                let new_path = format!("{}/{}", self.browser_cwd, entry);
                if std::path::Path::new(&new_path).is_dir() {
                    self.browser_cwd = new_path;
                    self.refresh_browser();
                }
                None
            }
        }
    }

    /// In browser mode, select the current directory as the project.
    pub fn select_current_dir(&self) -> Option<String> {
        if self.mode == PickerMode::DirectoryBrowser {
            Some(self.browser_cwd.clone())
        } else {
            None
        }
    }

    /// Go up one directory in browser mode.
    pub fn go_up(&mut self) {
        if self.mode == PickerMode::DirectoryBrowser {
            if let Some(parent) = std::path::Path::new(&self.browser_cwd)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
            {
                self.browser_cwd = parent;
                self.refresh_browser();
            }
        }
    }

    /// Go back to recent projects from browser.
    pub fn back_to_recent(&mut self) {
        self.mode = PickerMode::RecentProjects;
    }

    fn enter_browser(&mut self) {
        self.mode = PickerMode::DirectoryBrowser;
        self.browser_selected = 0;
        self.refresh_browser();
    }

    fn refresh_browser(&mut self) {
        self.browser_entries = projects::list_dirs(&self.browser_cwd);
        self.browser_selected = 0;
    }

    /// Shorten a path for display (replace home with ~).
    fn display_path(path: &str) -> String {
        if let Ok(home) = std::env::var("HOME") {
            if let Some(rest) = path.strip_prefix(&home) {
                return format!("~{rest}");
            }
        }
        path.to_string()
    }
}

pub fn render_project_picker(frame: &mut Frame, area: Rect, picker: &ProjectPicker) {
    match picker.mode {
        PickerMode::RecentProjects => render_recent_projects(frame, area, picker),
        PickerMode::DirectoryBrowser => render_directory_browser(frame, area, picker),
    }
}

fn render_recent_projects(frame: &mut Frame, area: Rect, picker: &ProjectPicker) {
    let block = Block::default()
        .title(" Select Project ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(2, 2, 1, 1));

    let mut items: Vec<ListItem> = picker
        .recent_projects
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let is_selected = i == picker.selected;
            let indicator = if is_selected { "▶ " } else { "  " };
            let display = ProjectPicker::display_path(path);
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::raw(indicator.to_string()),
                Span::styled(display, style),
            ]))
        })
        .collect();

    // "Browse..." option.
    let browse_selected = picker.selected == picker.recent_projects.len();
    let indicator = if browse_selected { "▶ " } else { "  " };
    let style = if browse_selected {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Yellow)
    };

    // Separator.
    items.push(ListItem::new(Line::from(Span::styled(
        "  ──────────────",
        Style::default().fg(Color::DarkGray),
    ))));
    items.push(ListItem::new(Line::from(vec![
        Span::raw(indicator.to_string()),
        Span::styled("Browse...", style),
    ])));

    let help = Line::from(Span::styled(
        "  ↑↓: select  Enter: confirm  Esc: cancel",
        Style::default().fg(Color::DarkGray),
    ));
    items.push(ListItem::new(Line::from("")));
    items.push(ListItem::new(help));

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

fn render_directory_browser(frame: &mut Frame, area: Rect, picker: &ProjectPicker) {
    let display_cwd = ProjectPicker::display_path(&picker.browser_cwd);
    let block = Block::default()
        .title(format!(" Browse: {display_cwd} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(2, 2, 1, 1));

    let mut items: Vec<ListItem> = picker
        .browser_entries
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let is_selected = i == picker.browser_selected;
            let indicator = if is_selected { "▶ " } else { "  " };
            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Line::from(vec![
                Span::raw(indicator.to_string()),
                Span::styled(format!("📁 {name}"), style),
            ]))
        })
        .collect();

    if picker.browser_entries.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (empty)",
            Style::default().fg(Color::DarkGray),
        ))));
    }

    items.push(ListItem::new(Line::from("")));
    items.push(ListItem::new(Line::from(Span::styled(
        "  ↑↓: select  Enter: open  Backspace: up  Space: choose this dir  Esc: back",
        Style::default().fg(Color::DarkGray),
    ))));

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

fn dirs_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}
