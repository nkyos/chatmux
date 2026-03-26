use crate::agent::{self, AgentKind, AgentRegistry};
use crate::config::{Config, ResolvedTheme};
use crate::session::model::SessionStatus;
use crate::session::state::HistoryEntry;
use crate::session::{SessionManager, SortMode};
use crate::tui::project_picker::{PickerMode, ProjectPicker, render_project_picker};
use crate::tui::render_startup_screen;
use crate::tui::sidebar::{
    render_history_sidebar, render_project_list, render_sidebar, render_sidebar_with_title,
    render_summary_bar, ProjectSummary,
};
use crate::tui::terminal::{render_empty_terminal, render_terminal};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::ListState,
    Frame,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Sidebar,
    Terminal,
    ProjectPicker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AppMode {
    /// Startup screen: ask user whether to restore or start fresh.
    Startup { existing_sessions: Vec<String> },
    /// Normal operation.
    Normal,
}

/// Which view the sidebar is currently showing.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SidebarView {
    /// Flat list of all sessions (default).
    Sessions,
    /// Grouped by project.
    Projects,
    /// Sessions within a specific project (cwd).
    ProjectSessions(String),
}

/// How often to poll JSONL files for status changes.
const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Time window for double-Esc detection.
const DOUBLE_ESC_WINDOW: Duration = Duration::from_millis(200);

pub struct App {
    config: Config,
    theme: ResolvedTheme,
    registry: AgentRegistry,
    manager: SessionManager,
    selected: Option<usize>,
    focus: Focus,
    mode: AppMode,
    should_quit: bool,
    /// If true, save state on quit instead of killing sessions.
    detach_on_quit: bool,
    terminal_content: String,
    /// Terminal scroll offset: lines scrolled back from the bottom (0 = live view).
    terminal_scroll: u16,
    picker: Option<ProjectPicker>,
    /// Cached terminal area for pane sizing.
    terminal_area: Rect,
    /// Cached sidebar area for mouse click detection.
    sidebar_area: Rect,
    /// Last time we checked JSONL files for status.
    last_status_poll: Instant,
    /// Pending Esc in terminal mode (for double-Esc detection).
    pending_esc: Option<Instant>,
    /// Current sort mode.
    sort_mode: SortMode,
    /// Rename mode: Some(buffer) when editing a session label.
    rename_buf: Option<String>,
    /// Filter mode: Some(filter_text) when filtering sessions.
    filter_input: Option<String>,
    /// History mode toggle.
    show_history: bool,
    /// Loaded history entries.
    history_entries: Vec<HistoryEntry>,
    /// Selected index in history view.
    history_selected: usize,
    /// Scroll state for sidebar session list.
    sidebar_list_state: ListState,
    /// Scroll state for history sidebar list.
    history_list_state: ListState,
    /// Cached project list (computed once at startup to avoid repeated I/O).
    cached_projects: Vec<String>,
    /// Current sidebar view mode.
    sidebar_view: SidebarView,
    /// Selected index in project list view.
    project_selected: usize,
    /// Scroll state for project list.
    project_list_state: ListState,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load();
        let theme = ResolvedTheme::from_config(&config.theme);
        let registry = AgentRegistry::new();
        let manager = SessionManager::new();
        let existing = manager.tmux().list_chatmux_sessions();
        let mode = if existing.is_empty() {
            AppMode::Normal
        } else {
            AppMode::Startup {
                existing_sessions: existing,
            }
        };

        // Build project list from chatmux's own history (no agent file scanning).
        // Falls back to agent discovery only on first use when history is empty.
        let mut cached_projects = Self::projects_from_history();
        if cached_projects.is_empty() {
            cached_projects = registry.discover_all_projects();
        }

        Self {
            config,
            theme,
            registry,
            manager,
            selected: None,
            focus: Focus::Sidebar,
            mode,
            should_quit: false,
            detach_on_quit: false,
            terminal_content: String::new(),
            terminal_scroll: 0,
            picker: None,
            terminal_area: Rect::default(),
            sidebar_area: Rect::default(),
            last_status_poll: Instant::now(),
            pending_esc: None,
            sort_mode: SortMode::StatusPriority,
            rename_buf: None,
            filter_input: None,
            show_history: false,
            history_entries: Vec::new(),
            history_selected: 0,
            sidebar_list_state: ListState::default(),
            history_list_state: ListState::default(),
            cached_projects,
            sidebar_view: SidebarView::Sessions,
            project_selected: 0,
            project_list_state: ListState::default(),
        }
    }

    /// Build project list from chatmux's own history + saved sessions.
    /// No agent file scanning needed — just reads chatmux's JSON files.
    fn projects_from_history() -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut projects: Vec<(String, u64)> = Vec::new();

        // From history (already sorted by most recent first).
        for entry in crate::session::state::load_history() {
            if seen.insert(entry.cwd.clone()) {
                projects.push((entry.cwd, entry.ended_at));
            }
        }

        // From saved sessions (currently live / detached).
        if let Some(saved) = crate::session::state::load() {
            for entry in saved.sessions {
                if seen.insert(entry.cwd.clone()) {
                    let epoch = entry.last_activity_epoch.unwrap_or(0);
                    projects.push((entry.cwd, epoch));
                }
            }
        }

        // Sort by most recent first.
        projects.sort_by(|a, b| b.1.cmp(&a.1));

        // Filter to directories that still exist.
        projects
            .into_iter()
            .filter(|(p, _)| std::path::Path::new(p).is_dir())
            .map(|(p, _)| p)
            .collect()
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    /// Called every frame before draw. Resizes tmux panes and captures content.
    pub fn tick(&mut self) {
        // Skip ticking during startup screen.
        if matches!(self.mode, AppMode::Startup { .. }) {
            return;
        }

        // Resize tmux panes to match the terminal view area.
        // Skip sessions with an attached external client (e.g. from `chatmux claude`).
        let pane_width = self.terminal_area.width.saturating_sub(2);
        let pane_height = self.terminal_area.height.saturating_sub(2);
        if pane_width > 0 && pane_height > 0 {
            for i in 0..self.manager.len() {
                if self.manager.get(i).is_some_and(|s| s.attached_externally) {
                    continue;
                }
                let _ = self.manager.resize(i, pane_width, pane_height);
            }
        }

        // Capture content for the selected session.
        if let Some(idx) = self.selected {
            if idx < self.manager.len() {
                let result = if self.terminal_scroll > 0 {
                    self.manager
                        .capture_scroll(idx, self.terminal_scroll, pane_height)
                } else {
                    self.manager.capture(idx)
                };
                if let Ok(content) = result {
                    self.terminal_content = content;
                }
            }
        }

        // Flush pending Esc if the double-Esc window has passed.
        if let Some(esc_time) = self.pending_esc {
            if esc_time.elapsed() >= DOUBLE_ESC_WINDOW {
                self.pending_esc = None;
                if let Some(idx) = self.selected {
                    let _ = self.manager.send_keys(idx, "Escape");
                }
            }
        }

        // Periodically poll session files to update session statuses.
        if self.last_status_poll.elapsed() >= STATUS_POLL_INTERVAL {
            self.last_status_poll = Instant::now();
            self.discover_external_sessions();
            self.poll_session_statuses();
            self.auto_sort();
        }
    }

    /// Discover chatmux tmux sessions created externally (e.g. via `chatmux claude`).
    /// Also removes sessions whose tmux session has died.
    fn discover_external_sessions(&mut self) {
        let live: std::collections::HashSet<String> = self
            .manager
            .tmux()
            .list_chatmux_sessions()
            .into_iter()
            .collect();

        // Remove dead sessions (tmux session no longer exists).
        let had_dead = self.manager.sessions().iter().any(|s| !live.contains(&s.name));
        if had_dead {
            // Record dead sessions in history before removing.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            for session in self.manager.sessions() {
                if !live.contains(&session.name) {
                    let entry = crate::session::state::HistoryEntry {
                        cwd: session.cwd.clone(),
                        project_name: session.project_name.clone(),
                        agent_kind: session.agent_kind,
                        task_label: session.task_label.clone(),
                        last_prompt: session.last_prompt.clone(),
                        ended_at: now,
                    };
                    crate::session::state::append_history(&entry);
                }
            }

            // Get the currently selected session name before removal.
            let selected_name = self
                .selected
                .and_then(|idx| self.manager.get(idx))
                .map(|s| s.name.clone());

            self.manager.sessions_mut().retain(|s| live.contains(&s.name));

            // Fix up selected index after removal.
            if let Some(name) = selected_name {
                self.selected = self
                    .manager
                    .sessions()
                    .iter()
                    .position(|s| s.name == name);
            }
            if self.selected.is_none() && !self.manager.is_empty() {
                self.selected = Some(0);
            }
        }

        let tracked: std::collections::HashSet<String> = self
            .manager
            .sessions()
            .iter()
            .map(|s| s.name.clone())
            .collect();

        // Collect info for new sessions first (avoids borrow conflicts).
        let new_sessions: Vec<_> = live.iter()
            .filter(|name| !tracked.contains(*name))
            .map(|name| {
                let cwd = self
                    .manager
                    .tmux()
                    .get_pane_cwd(name)
                    .unwrap_or_else(|| "/".to_string());

                // Try saved state for agent kind, then detect from pane command.
                let agent_kind = self.agent_kind_from_state(name)
                    .or_else(|| self.detect_agent_kind(name))
                    .unwrap_or_default();

                let has_client = self.manager.tmux().has_attached_client(name);
                let created_epoch = self.manager.tmux().get_session_created(name);
                (name.clone(), cwd, agent_kind, has_client, created_epoch)
            })
            .collect();

        for (name, cwd, agent_kind, has_client, created_epoch) in new_sessions {
            let mut session =
                crate::session::Session::new(name.clone(), cwd.clone(), agent_kind);
            session.attached_externally = has_client;

            // For externally discovered sessions, compute pre_existing_files
            // using the tmux session's creation time. Files modified before the
            // session was created cannot belong to this session.
            if let Some(epoch) = created_epoch {
                let created_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch);
                let agent = self.registry.get(agent_kind);
                session.pre_existing_files = agent
                    .list_session_files(&cwd)
                    .into_iter()
                    .filter(|p| {
                        p.metadata()
                            .ok()
                            .and_then(|m| m.modified().ok())
                            .is_some_and(|t| t < created_time)
                    })
                    .collect();
            }

            self.manager.sessions_mut().push(session);

            if let Some(num) = name.strip_prefix('s').and_then(|n| n.parse::<usize>().ok()) {
                self.manager.ensure_next_id(num + 1);
            }
        }

        // Update attached_externally flag for all tracked sessions.
        let attach_status: Vec<_> = self.manager.sessions()
            .iter()
            .map(|s| self.manager.tmux().has_attached_client(&s.name))
            .collect();
        for (session, attached) in self.manager.sessions_mut().iter_mut().zip(attach_status) {
            session.attached_externally = attached;
        }
    }

    /// Try to determine agent kind from saved sessions.json.
    fn agent_kind_from_state(&self, name: &str) -> Option<AgentKind> {
        let saved = crate::session::state::load()?;
        saved.sessions.iter()
            .find(|e| e.name == name)
            .map(|e| e.agent_kind)
    }

    /// Detect agent kind by checking the process tree inside the tmux pane.
    /// `pane_current_command` may return the parent shell (e.g. "fish"),
    /// so we also check the pane's full command line via pane_start_command.
    fn detect_agent_kind(&self, name: &str) -> Option<AgentKind> {
        // First try pane_current_command (works when claude/codex is foreground).
        if let Some(cmd) = self.manager.tmux().get_pane_command(name) {
            match cmd.as_str() {
                "claude" => return Some(AgentKind::ClaudeCode),
                "codex" => return Some(AgentKind::Codex),
                _ => {}
            }
        }
        // Fall back: check the original start command of the pane.
        if let Some(start_cmd) = self.manager.tmux().get_pane_start_command(name) {
            if start_cmd.contains("claude") {
                return Some(AgentKind::ClaudeCode);
            }
            if start_cmd.contains("codex") {
                return Some(AgentKind::Codex);
            }
        }
        None
    }

    /// Check session files for each session and update their status via agent adapters.
    fn poll_session_statuses(&mut self) {
        let notifications_enabled = self.config.notifications.enabled;
        let notify_statuses = self.config.notifications.statuses.clone();
        let sound = self.config.notifications.sound.clone();

        // Collect already-assigned JSONL paths and accumulate new assignments
        // during the loop to prevent multiple sessions from claiming the same file.
        let mut assigned: Vec<std::path::PathBuf> = self
            .manager
            .sessions()
            .iter()
            .filter_map(|s| s.jsonl_path.clone())
            .collect();

        let registry = &self.registry;

        for session in self.manager.sessions_mut() {
            let agent_adapter = registry.get(session.agent_kind);

            // Lazily resolve the session file path.
            // Exclude pre-existing files and files already assigned to other sessions.
            if session.jsonl_path.is_none() {
                let mut exclude = session.pre_existing_files.clone();
                exclude.extend(assigned.iter().cloned());
                session.jsonl_path = agent_adapter.find_session_file(&session.cwd, &exclude);
                // Track newly assigned path so subsequent sessions won't claim the same file.
                if let Some(ref path) = session.jsonl_path {
                    assigned.push(path.clone());
                }
            }

            let Some(ref jsonl_path) = session.jsonl_path else {
                continue;
            };

            // Only re-read if the file has been modified since last check.
            // Compare at second granularity because restored values lose
            // sub-second precision.
            let current_modified = agent::file_modified(jsonl_path);
            let current_secs = current_modified
                .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            let saved_secs = session.jsonl_modified
                .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            if current_secs == saved_secs && saved_secs.is_some() {
                continue;
            }
            session.jsonl_modified = current_modified;

            // Detect status from the session file.
            if let Some(detected) = agent_adapter.detect_status(jsonl_path) {
                let old_status = session.status.clone();
                if old_status != detected.status {
                    session.status = detected.status.clone();
                    session.touch_activity();

                    // Send notification if this status is in the notify list.
                    if notifications_enabled
                        && notify_statuses.contains(&detected.status.name().to_string())
                    {
                        crate::notify::notify_status(
                            &session.project_name,
                            &format!("{} {}", session.agent_kind.label(), detected.status.name()),
                            &sound,
                        );
                    }
                }
                // Update last prompt if changed.
                if detected.last_prompt.is_some() && detected.last_prompt != session.last_prompt {
                    session.last_prompt = detected.last_prompt;
                }
            }
        }
    }

    /// Apply the current sort mode to the session list.
    fn auto_sort(&mut self) {
        if self.manager.is_empty() {
            return;
        }
        // Remember selected session name to preserve selection after sort.
        let selected_name = self
            .selected
            .and_then(|i| self.manager.get(i))
            .map(|s| s.name.clone());

        match self.sort_mode {
            SortMode::StatusPriority => {
                self.manager.sort_by_priority();
            }
            SortMode::LastActivity => {
                self.manager.sort_by_activity();
            }
            SortMode::Manual => {}
        }

        // Restore selection by name.
        if let Some(name) = selected_name {
            self.selected = self
                .manager
                .sessions()
                .iter()
                .position(|s| s.name == name);
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

    /// Build project summaries from current sessions, grouped by cwd.
    fn build_project_summaries(&self) -> Vec<ProjectSummary> {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<String, ProjectSummary> = BTreeMap::new();

        for session in self.manager.sessions() {
            let entry = map.entry(session.cwd.clone()).or_insert_with(|| {
                ProjectSummary {
                    cwd: session.cwd.clone(),
                    project_name: session.project_name.clone(),
                    session_count: 0,
                    has_replied: false,
                    has_working: false,
                    aggregate_status: SessionStatus::Read,
                    latest_activity_epoch: 0,
                }
            });
            entry.session_count += 1;
            if session.last_activity_epoch > entry.latest_activity_epoch {
                entry.latest_activity_epoch = session.last_activity_epoch;
            }
            match session.status {
                SessionStatus::Replied => {
                    entry.has_replied = true;
                    entry.aggregate_status = SessionStatus::Replied;
                }
                SessionStatus::Working => {
                    entry.has_working = true;
                    if entry.aggregate_status != SessionStatus::Replied {
                        entry.aggregate_status = SessionStatus::Working;
                    }
                }
                SessionStatus::Read => {}
            }
        }

        let mut summaries: Vec<ProjectSummary> = map.into_values().collect();
        // Sort: replied first, then working, then read; within same status by latest activity.
        summaries.sort_by(|a, b| {
            a.aggregate_status
                .sort_priority()
                .cmp(&b.aggregate_status.sort_priority())
                .then(b.latest_activity_epoch.cmp(&a.latest_activity_epoch))
        });
        summaries
    }

    /// Compute visible session indices for a specific project (cwd).
    fn project_session_indices(&self, project_cwd: &str) -> Vec<usize> {
        self.manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.cwd == project_cwd)
            .map(|(i, _)| i)
            .collect()
    }

    /// Compute visible session indices based on current filter.
    fn visible_indices(&self) -> Vec<usize> {
        let Some(ref filter) = self.filter_input else {
            return (0..self.manager.len()).collect();
        };
        if filter.is_empty() {
            return (0..self.manager.len()).collect();
        }
        let filter_lower = filter.to_lowercase();
        self.manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.project_name.to_lowercase().contains(&filter_lower)
                    || s.cwd.to_lowercase().contains(&filter_lower)
                    || s.task_label
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&filter_lower)
                    || s.status.name().contains(&filter_lower)
            })
            .map(|(i, _)| i)
            .collect()
    }

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

        // Sidebar: list + summary bar.
        let sidebar_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
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

        // Right pane: project picker or terminal.
        if let Some(ref picker) = self.picker {
            render_project_picker(frame, chunks[1], picker);
        } else if self.selected.is_some() {
            let label = self
                .selected
                .and_then(|i| self.manager.get(i))
                .map(|s| {
                    let base = s.display_label().to_string();
                    if self.terminal_scroll > 0 {
                        format!("{base} [scroll: -{}]", self.terminal_scroll)
                    } else {
                        base
                    }
                });
            render_terminal(
                frame,
                chunks[1],
                &self.terminal_content,
                label.as_deref(),
                self.focus == Focus::Terminal,
                &self.theme,
            );
        } else {
            render_empty_terminal(frame, chunks[1], &self.theme);
        }
    }

    pub fn handle_event(&mut self) -> Result<()> {
        if event::poll(Duration::from_millis(100))? {
            let ev = event::read()?;
            match ev {
                Event::Key(key) => {
                    if matches!(self.mode, AppMode::Startup { .. }) {
                        self.handle_startup_key(key.code)?;
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
                if !self.is_in_sidebar(x, y) {
                    return Ok(());
                }
                let content_y = (y - self.sidebar_area.y - 1) as usize;

                if self.show_history {
                    // History items are 2 lines each; account for scroll offset.
                    let offset = self.history_list_state.offset();
                    let idx = self.item_index_at_y(content_y, offset, 2);
                    if idx < self.history_entries.len() {
                        self.history_selected = idx;
                    }
                } else {
                    let offset = self.sidebar_list_state.offset();
                    let has_filter = self.filter_input.is_some();

                    // Walk items from offset, accumulating line heights.
                    let visible = self.visible_indices();
                    let filter_items = if has_filter { 2 } else { 0 };
                    let total_items = filter_items + visible.len();
                    let mut y_accum = 0usize;

                    for item_idx in offset..total_items {
                        let height = if has_filter && item_idx < 2 { 1 } else { 3 };
                        if content_y < y_accum + height {
                            // Clicked on filter bar — ignore.
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
            MouseEventKind::ScrollDown => {
                if self.is_in_sidebar(x, y) {
                    if self.show_history {
                        if !self.history_entries.is_empty() {
                            self.history_selected =
                                (self.history_selected + 1).min(self.history_entries.len() - 1);
                        }
                    } else {
                        self.select_next();
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
                        self.select_prev();
                    }
                } else if self.is_in_terminal(x, y) {
                    if let Some(idx) = self.selected {
                        let max = self.manager.history_size(idx);
                        self.terminal_scroll =
                            self.terminal_scroll.saturating_add(3).min(max);
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

    /// Ensure the selected session is within the visible (filtered) list.
    fn ensure_selected_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(sel) = self.selected {
            if !visible.contains(&sel) {
                self.selected = Some(visible[0]);
            }
        }
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
                    _ => {}
                }
            }
            SidebarView::Sessions => {
                // Should not reach here, but handle gracefully.
            }
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
            _ => {}
        }
        Ok(())
    }

    fn handle_terminal_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        // Any key input in terminal resets scroll to live view.
        self.terminal_scroll = 0;

        // If a non-Esc key arrives while Esc is pending, flush the pending Esc first.
        if code != KeyCode::Esc {
            if self.pending_esc.take().is_some() {
                if let Some(idx) = self.selected {
                    self.manager.send_keys(idx, "Escape")?;
                }
            }
        }

        if code == KeyCode::Esc {
            if self.pending_esc.is_some() {
                // Double Esc: go to sidebar.
                self.pending_esc = None;
                self.focus = Focus::Sidebar;
            } else {
                // First Esc: wait for possible second Esc.
                self.pending_esc = Some(Instant::now());
            }
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

    fn create_session(&mut self, path: &str, agent_kind: AgentKind) -> Result<()> {
        let agent = self.registry.get(agent_kind);
        // Use the terminal area size (minus borders) for the tmux pane.
        let width = self.terminal_area.width.saturating_sub(2);
        let height = self.terminal_area.height.saturating_sub(2);
        let idx = self.manager.create(path, agent, width, height)?;
        self.selected = Some(idx);
        self.picker = None;
        self.focus = Focus::Sidebar;
        // Add newly used path to cache if not already present.
        if !self.cached_projects.contains(&path.to_string()) {
            self.cached_projects.insert(0, path.to_string());
        }
        Ok(())
    }

    fn select_next(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let old = self.selected;
        self.selected = Some(match self.selected {
            Some(current) => {
                if let Some(pos) = visible.iter().position(|&i| i == current) {
                    visible[(pos + 1).min(visible.len() - 1)]
                } else {
                    visible[0]
                }
            }
            None => visible[0],
        });
        if self.selected != old {
            self.terminal_scroll = 0;
        }
    }

    fn select_prev(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        let old = self.selected;
        self.selected = Some(match self.selected {
            Some(current) => {
                if let Some(pos) = visible.iter().position(|&i| i == current) {
                    visible[pos.saturating_sub(1)]
                } else {
                    visible[0]
                }
            }
            None => visible[0],
        });
        if self.selected != old {
            self.terminal_scroll = 0;
        }
    }

    /// If the selected session is Replied, mark it as Read.
    fn mark_selected_as_read(&mut self) {
        if let Some(idx) = self.selected {
            if let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                if session.status == crate::session::SessionStatus::Replied {
                    session.status = crate::session::SessionStatus::Read;
                }
            }
        }
    }

    /// Open the project directory in the configured editor.
    fn open_editor(&self, cwd: &str) -> Result<()> {
        let editor = self.config.editor_command();

        // Spawn detached — works for GUI editors (code, cursor, zed, etc.).
        std::process::Command::new(&editor)
            .arg(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok();
        Ok(())
    }

    pub fn cleanup(&mut self) {
        if self.detach_on_quit {
            self.manager.detach();
        } else {
            self.manager.cleanup();
        }
    }
}
