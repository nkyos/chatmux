use crate::agent::AgentKind;
use crate::projects;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Padding},
    Frame,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerMode {
    RecentProjects,
    DirectoryBrowser,
    AgentSelect { path: String },
}

/// Result of the picker flow: project path + agent kind.
pub struct PickerResult {
    pub path: String,
    pub agent_kind: AgentKind,
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
    /// Filter input for incremental search.
    pub filter_input: String,
    /// Available agents (populated lazily for agent selection).
    pub available_agents: Vec<AgentKind>,
    /// Agent selection index.
    pub agent_selected: usize,
}

impl ProjectPicker {
    pub fn new(available_agents: Vec<AgentKind>, all_projects: Vec<String>) -> Self {
        let recent_projects = all_projects;
        Self {
            mode: PickerMode::RecentProjects,
            recent_projects,
            selected: 0,
            browser_cwd: dirs_home(),
            browser_entries: Vec::new(),
            browser_selected: 0,
            filter_input: String::new(),
            available_agents,
            agent_selected: 0,
        }
    }

    /// Return recent projects matching the current filter (case-insensitive substring).
    pub fn filtered_recent_projects(&self) -> Vec<String> {
        if self.filter_input.is_empty() {
            self.recent_projects.clone()
        } else {
            let lower = self.filter_input.to_lowercase();
            self.recent_projects
                .iter()
                .filter(|p| p.to_lowercase().contains(&lower))
                .cloned()
                .collect()
        }
    }

    /// Return browser entries matching the current filter.
    /// When filter is active, recursively searches subdirectories (up to depth 4).
    pub fn filtered_browser_entries(&self) -> Vec<String> {
        if self.filter_input.is_empty() {
            self.browser_entries.clone()
        } else {
            projects::find_dirs_recursive(&self.browser_cwd, &self.filter_input, 4, 50)
        }
    }

    /// Append a character to the filter and reset selection.
    pub fn on_char(&mut self, c: char) {
        if matches!(self.mode, PickerMode::AgentSelect { .. }) {
            return; // No text input in agent select mode
        }
        self.filter_input.push(c);
        match self.mode {
            PickerMode::RecentProjects => self.selected = 0,
            PickerMode::DirectoryBrowser => self.browser_selected = 0,
            PickerMode::AgentSelect { .. } => {}
        }
    }

    /// Remove the last character from the filter. Returns true if consumed.
    pub fn on_backspace_filter(&mut self) -> bool {
        if matches!(self.mode, PickerMode::AgentSelect { .. }) {
            return false;
        }
        if self.filter_input.pop().is_some() {
            match self.mode {
                PickerMode::RecentProjects => self.selected = 0,
                PickerMode::DirectoryBrowser => self.browser_selected = 0,
                PickerMode::AgentSelect { .. } => {}
            }
            true
        } else {
            false
        }
    }

    /// Returns true if the filter is non-empty.
    pub fn has_filter(&self) -> bool {
        !self.filter_input.is_empty()
    }

    /// Clear the filter and reset selection.
    pub fn clear_filter(&mut self) {
        if !self.filter_input.is_empty() {
            self.filter_input.clear();
            match self.mode {
                PickerMode::RecentProjects => self.selected = 0,
                PickerMode::DirectoryBrowser => self.browser_selected = 0,
                PickerMode::AgentSelect { .. } => {}
            }
        }
    }

    pub fn move_down(&mut self) {
        match self.mode {
            PickerMode::RecentProjects => {
                let total = self.filtered_recent_projects().len() + 1; // +1 for Browse
                if total > 0 {
                    self.selected = (self.selected + 1) % total;
                }
            }
            PickerMode::DirectoryBrowser => {
                let count = self.filtered_browser_entries().len();
                if count > 0 {
                    self.browser_selected = (self.browser_selected + 1) % count;
                }
            }
            PickerMode::AgentSelect { .. } => {
                let count = self.available_agents.len();
                if count > 0 {
                    self.agent_selected = (self.agent_selected + 1) % count;
                }
            }
        }
    }

    pub fn move_up(&mut self) {
        match self.mode {
            PickerMode::RecentProjects => {
                let total = self.filtered_recent_projects().len() + 1; // +1 for Browse
                if total > 0 {
                    self.selected = if self.selected == 0 { total - 1 } else { self.selected - 1 };
                }
            }
            PickerMode::DirectoryBrowser => {
                let count = self.filtered_browser_entries().len();
                if count > 0 {
                    self.browser_selected = if self.browser_selected == 0 { count - 1 } else { self.browser_selected - 1 };
                }
            }
            PickerMode::AgentSelect { .. } => {
                let count = self.available_agents.len();
                if count > 0 {
                    self.agent_selected = if self.agent_selected == 0 { count - 1 } else { self.agent_selected - 1 };
                }
            }
        }
    }

    /// Handle Enter. Returns Some(PickerResult) when the full flow completes.
    pub fn confirm(&mut self) -> Option<PickerResult> {
        match self.mode.clone() {
            PickerMode::RecentProjects => {
                let filtered = self.filtered_recent_projects();
                if self.selected < filtered.len() {
                    let path = filtered[self.selected].clone();
                    self.on_project_selected(path)
                } else {
                    // Selected "Browse...".
                    self.enter_browser();
                    None
                }
            }
            PickerMode::DirectoryBrowser => {
                let filtered = self.filtered_browser_entries();
                if filtered.is_empty() || self.browser_selected >= filtered.len() {
                    return None;
                }
                let entry = filtered[self.browser_selected].clone();
                let new_path = format!("{}/{}", self.browser_cwd, entry);
                if std::path::Path::new(&new_path).is_dir() {
                    self.browser_cwd = new_path;
                    self.refresh_browser();
                }
                None
            }
            PickerMode::AgentSelect { path } => {
                if self.agent_selected < self.available_agents.len() {
                    let agent_kind = self.available_agents[self.agent_selected];
                    Some(PickerResult { path, agent_kind })
                } else {
                    None
                }
            }
        }
    }

    /// In browser mode, select the current directory as the project.
    pub fn select_current_dir(&mut self) -> Option<PickerResult> {
        if self.mode == PickerMode::DirectoryBrowser {
            let path = self.browser_cwd.clone();
            self.on_project_selected(path)
        } else {
            None
        }
    }

    /// Called when a project path is selected. Enters agent selection if needed.
    fn on_project_selected(&mut self, path: String) -> Option<PickerResult> {
        if self.available_agents.len() <= 1 {
            // Only one (or zero) agents — skip selection.
            let agent_kind = self
                .available_agents
                .first()
                .copied()
                .unwrap_or_default();
            Some(PickerResult { path, agent_kind })
        } else {
            self.mode = PickerMode::AgentSelect { path };
            self.agent_selected = 0;
            self.filter_input.clear();
            None
        }
    }

    /// Go back from agent selection to the previous project selection mode.
    pub fn back_from_agent_select(&mut self) {
        // Return to RecentProjects as default.
        self.mode = PickerMode::RecentProjects;
        self.filter_input.clear();
        self.selected = 0;
    }

    /// Go up one directory in browser mode.
    pub fn go_up(&mut self) {
        if self.mode == PickerMode::DirectoryBrowser
            && let Some(parent) = std::path::Path::new(&self.browser_cwd)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
            {
                self.browser_cwd = parent;
                self.refresh_browser();
            }
    }

    /// Go back to recent projects from browser.
    pub fn back_to_recent(&mut self) {
        self.mode = PickerMode::RecentProjects;
        self.filter_input.clear();
        self.selected = 0;
    }

    fn enter_browser(&mut self) {
        self.mode = PickerMode::DirectoryBrowser;
        self.browser_selected = 0;
        self.filter_input.clear();
        self.refresh_browser();
    }

    fn refresh_browser(&mut self) {
        self.browser_entries = projects::list_dirs(&self.browser_cwd);
        self.browser_selected = 0;
        self.filter_input.clear();
    }

    /// Shorten a path for display (replace home with ~).
    fn display_path(path: &str) -> String {
        if let Ok(home) = std::env::var("HOME")
            && let Some(rest) = path.strip_prefix(&home) {
                return format!("~{rest}");
            }
        path.to_string()
    }
}

pub fn render_project_picker(frame: &mut Frame, area: Rect, picker: &ProjectPicker) {
    match picker.mode {
        PickerMode::RecentProjects => render_recent_projects(frame, area, picker),
        PickerMode::DirectoryBrowser => render_directory_browser(frame, area, picker),
        PickerMode::AgentSelect { .. } => render_agent_select(frame, area, picker),
    }
}

fn render_recent_projects(frame: &mut Frame, area: Rect, picker: &ProjectPicker) {
    let block = Block::default()
        .title(" Select Project ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(2, 2, 1, 1));

    let filtered = picker.filtered_recent_projects();
    let mut items: Vec<ListItem> = Vec::new();

    // Show filter bar when active.
    if !picker.filter_input.is_empty() {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("  / ", Style::default().fg(Color::Yellow)),
            Span::styled(
                &picker.filter_input,
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ])));
        items.push(ListItem::new(Line::from("")));
    }

    for (i, path) in filtered.iter().enumerate() {
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
        items.push(ListItem::new(Line::from(vec![
            Span::raw(indicator.to_string()),
            Span::styled(display, style),
        ])));
    }

    // "Browse..." option.
    let browse_selected = picker.selected == filtered.len();
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
        "  ↑↓: select  type: filter  Enter: new  ^R: resume  Esc: cancel",
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

    let filtered = picker.filtered_browser_entries();
    let mut items: Vec<ListItem> = Vec::new();

    // Show filter bar when active.
    if !picker.filter_input.is_empty() {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("  / ", Style::default().fg(Color::Yellow)),
            Span::styled(
                &picker.filter_input,
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ])));
        items.push(ListItem::new(Line::from("")));
    }

    for (i, name) in filtered.iter().enumerate() {
        let is_selected = i == picker.browser_selected;
        let indicator = if is_selected { "▶ " } else { "  " };
        let style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        items.push(ListItem::new(Line::from(vec![
            Span::raw(indicator.to_string()),
            Span::styled(name.to_string(), style),
        ])));
    }

    if filtered.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no match)",
            Style::default().fg(Color::DarkGray),
        ))));
    }

    items.push(ListItem::new(Line::from("")));
    items.push(ListItem::new(Line::from(Span::styled(
        "  ↑↓: select  type: filter  Enter: open  Backspace: up  Space: choose  Esc: back",
        Style::default().fg(Color::DarkGray),
    ))));

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

fn render_agent_select(frame: &mut Frame, area: Rect, picker: &ProjectPicker) {
    let path_display = if let PickerMode::AgentSelect { ref path } = picker.mode {
        ProjectPicker::display_path(path)
    } else {
        String::new()
    };

    let block = Block::default()
        .title(format!(" Select Agent — {path_display} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::new(2, 2, 1, 1));

    let mut items: Vec<ListItem> = Vec::new();

    for (i, agent_kind) in picker.available_agents.iter().enumerate() {
        let is_selected = i == picker.agent_selected;
        let indicator = if is_selected { "▶ " } else { "  " };
        let icon = agent_kind.icon();
        let label = agent_kind.label();
        let style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let icon_color = agent_kind.icon_color();
        items.push(ListItem::new(Line::from(vec![
            Span::raw(indicator.to_string()),
            Span::styled(format!("{icon} "), Style::default().fg(icon_color)),
            Span::styled(label, style),
        ])));
    }

    items.push(ListItem::new(Line::from("")));
    items.push(ListItem::new(Line::from(Span::styled(
        "  ↑↓: select  Enter: confirm  Esc: back",
        Style::default().fg(Color::DarkGray),
    ))));

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

fn dirs_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
}
