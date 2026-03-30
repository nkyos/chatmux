mod input;
mod render;
mod session_sync;

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
use crate::tui::help::{HelpContext, render_confirm_overlay, render_help_overlay};
use crate::tui::terminal::{render_empty_terminal, render_terminal};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    widgets::ListState,
    Frame,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Text selection in the terminal area (content coordinates relative to inner area).
#[derive(Debug, Clone, Copy)]
pub(super) struct Selection {
    /// Start position (row, col) — where the mouse was pressed.
    pub(super) start: (u16, u16),
    /// Current end position (row, col) — where the mouse is now.
    pub(super) end: (u16, u16),
}

impl Selection {
    /// Return (top, bottom) with each as (row, col), ensuring top <= bottom.
    pub(super) fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        if self.start.0 < self.end.0
            || (self.start.0 == self.end.0 && self.start.1 <= self.end.1)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Sidebar,
    Terminal,
    ProjectPicker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AppMode {
    /// Startup screen: ask user whether to restore or start fresh.
    Startup { existing_sessions: Vec<String> },
    /// Normal operation.
    Normal,
}

#[derive(Debug, Clone)]
pub(super) struct RestartEntry {
    pub(super) cwd: String,
    pub(super) agent_kind: AgentKind,
    pub(super) task_label: Option<String>,
    pub(super) agent_session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConfirmAction {
    UpgradeAndRestart,
    RestartAll,
}

/// Which view the sidebar is currently showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SidebarView {
    /// Flat list of all sessions (default).
    Sessions,
    /// Grouped by project.
    Projects,
    /// Sessions within a specific project (cwd).
    ProjectSessions(String),
}

/// How often to do a full poll of all JSONL files (fallback for watcher misses).
const STATUS_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// How often to check the watcher's dirty set for changed files.
const WATCHER_CHECK_INTERVAL: Duration = Duration::from_millis(300);

/// Check if a key event is the prefix key (Ctrl+]).
/// Legacy terminals send Ctrl+] as Ctrl+5; modern (Kitty protocol) sends Ctrl+].
pub(super) fn is_prefix_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char(']') | KeyCode::Char('5'))
}

pub struct App {
    pub(super) config: Config,
    pub(super) theme: ResolvedTheme,
    pub(super) registry: AgentRegistry,
    pub(super) manager: SessionManager,
    pub(super) selected: Option<usize>,
    pub(super) focus: Focus,
    pub(super) mode: AppMode,
    pub(super) should_quit: bool,
    /// If true, save state on quit instead of killing sessions.
    pub(super) detach_on_quit: bool,
    pub(super) terminal_content: String,
    /// Terminal scroll offset: lines scrolled back from the bottom (0 = live view).
    pub(super) terminal_scroll: u16,
    pub(super) picker: Option<ProjectPicker>,
    /// Cached terminal area for pane sizing.
    pub(super) terminal_area: Rect,
    /// Cached sidebar area for mouse click detection.
    pub(super) sidebar_area: Rect,
    /// Last time we checked JSONL files for status.
    pub(super) last_status_poll: Instant,
    /// Prefix mode active (Ctrl+] was pressed, waiting for next key).
    pub(super) prefix_active: bool,
    /// Current sort mode.
    pub(super) sort_mode: SortMode,
    /// Rename mode: Some(buffer) when editing a session label.
    pub(super) rename_buf: Option<String>,
    /// Filter mode: Some(filter_text) when filtering sessions.
    pub(super) filter_input: Option<String>,
    /// History mode toggle.
    pub(super) show_history: bool,
    /// Loaded history entries.
    pub(super) history_entries: Vec<HistoryEntry>,
    /// Selected index in history view.
    pub(super) history_selected: usize,
    /// Scroll state for sidebar session list.
    pub(super) sidebar_list_state: ListState,
    /// Scroll state for history sidebar list.
    pub(super) history_list_state: ListState,
    /// Cached project list (computed once at startup to avoid repeated I/O).
    pub(super) cached_projects: Vec<String>,
    /// Current sidebar view mode.
    pub(super) sidebar_view: SidebarView,
    /// Selected index in project list view.
    pub(super) project_selected: usize,
    /// Scroll state for project list.
    pub(super) project_list_state: ListState,
    /// Whether the help overlay is shown.
    pub(super) show_help: bool,
    /// Active text selection in the terminal area.
    pub(super) selection: Option<Selection>,
    /// Set of paths dirtied by the file watcher (shared with watcher thread).
    pub(super) watcher_dirty: Arc<Mutex<HashSet<PathBuf>>>,
    /// Keep the watcher alive (dropped on App drop).
    pub(super) _watcher: Option<notify::RecommendedWatcher>,
    /// Last time we checked the watcher dirty set.
    pub(super) last_watcher_check: Instant,
    /// Pending confirm action (U or R key).
    pub(super) confirm_action: Option<ConfirmAction>,
    /// Snapshot of sessions for restart/upgrade.
    pub(super) restart_snapshot: Vec<RestartEntry>,
    /// True while an upgrade tmux session is running.
    pub(super) upgrading: bool,
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

        let mut result = Self {
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
            prefix_active: false,
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
            show_help: false,
            selection: None,
            watcher_dirty: Arc::new(Mutex::new(HashSet::new())),
            _watcher: None,
            last_watcher_check: Instant::now(),
            confirm_action: None,
            restart_snapshot: Vec::new(),
            upgrading: false,
        };
        result.start_watcher();
        result
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

        // Monitor upgrade session progress.
        if self.upgrading {
            if let Ok(content) = self.manager.tmux().capture_pane("upgrade") {
                self.terminal_content = content;
            }
            if self.manager.tmux().is_pane_dead("upgrade") {
                let _ = self.finish_upgrade();
            }
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

        // Check watcher for dirty JSONL files and poll affected sessions.
        if self.last_watcher_check.elapsed() >= WATCHER_CHECK_INTERVAL {
            self.last_watcher_check = Instant::now();
            let dirty: HashSet<PathBuf> = {
                let mut set = self.watcher_dirty.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *set)
            };
            if !dirty.is_empty() {
                if let Some(name) = self.poll_dirty_sessions(&dirty) {
                    self.focus_session_by_name(&name);
                }
                self.auto_sort();
            }
        }

        // Periodically do a full poll as fallback (catches watcher misses).
        if self.last_status_poll.elapsed() >= STATUS_POLL_INTERVAL {
            self.last_status_poll = Instant::now();
            self.discover_external_sessions();
            if let Some(name) = self.poll_session_statuses() {
                self.focus_session_by_name(&name);
            }
            self.auto_sort();
        }
    }

    /// Build project summaries from current sessions, grouped by cwd.
    pub(super) fn build_project_summaries(&self) -> Vec<ProjectSummary> {
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
    pub(super) fn project_session_indices(&self, project_cwd: &str) -> Vec<usize> {
        self.manager
            .sessions()
            .iter()
            .enumerate()
            .filter(|(_, s)| s.cwd == project_cwd)
            .map(|(i, _)| i)
            .collect()
    }

    /// Compute visible session indices based on current filter.
    pub(super) fn visible_indices(&self) -> Vec<usize> {
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

    pub(super) fn select_next(&mut self) {
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

    pub(super) fn select_prev(&mut self) {
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
    pub(super) fn mark_selected_as_read(&mut self) {
        if let Some(idx) = self.selected {
            if let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                if session.status == crate::session::SessionStatus::Replied {
                    session.status = crate::session::SessionStatus::Read;
                }
            }
        }
    }

    /// Ensure the selected session is within the visible (filtered) list.
    pub(super) fn ensure_selected_visible(&mut self) {
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

    /// Auto-focus a session by name when it transitions to Working.
    /// Only switches focus if the user is not already in Terminal mode (to avoid disrupting typing).
    fn focus_session_by_name(&mut self, name: &str) {
        if self.focus == Focus::Terminal {
            return;
        }
        if let Some(idx) = self
            .manager
            .sessions()
            .iter()
            .position(|s| s.name == name)
        {
            self.selected = Some(idx);
            self.focus = Focus::Terminal;
        }
    }

    pub(super) fn create_session(&mut self, path: &str, agent_kind: AgentKind) -> Result<()> {
        let agent = self.registry.get(agent_kind);
        // Use the terminal area size (minus borders) for the tmux pane.
        let width = self.terminal_area.width.saturating_sub(2);
        let height = self.terminal_area.height.saturating_sub(2);
        let idx = self.manager.create(path, agent, width, height)?;
        self.selected = Some(idx);
        self.picker = None;
        self.focus = Focus::Terminal;
        // Add newly used path to cache if not already present.
        if !self.cached_projects.contains(&path.to_string()) {
            self.cached_projects.insert(0, path.to_string());
        }
        Ok(())
    }

    /// Open the project directory in the configured editor.
    pub(super) fn open_editor(&self, cwd: &str) -> Result<()> {
        let (program, args) = self.config.editor_command_parts();

        // Spawn detached — works for GUI editors (code, cursor, zed, etc.).
        std::process::Command::new(&program)
            .args(&args)
            .arg(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok();
        Ok(())
    }

    /// Extract plain text lines from the terminal content (ANSI stripped).
    pub(super) fn plain_lines(&self) -> Vec<String> {
        use ansi_to_tui::IntoText;
        let text = self
            .terminal_content
            .as_bytes()
            .into_text()
            .unwrap_or_else(|_| ratatui::text::Text::raw(&self.terminal_content));
        text.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    /// Copy the selected text to the system clipboard via OSC 52.
    pub(super) fn copy_selection_to_clipboard(&self, sel: &Selection) {
        let lines = self.plain_lines();
        let ((r1, c1), (r2, c2)) = sel.ordered();
        let (r1, c1, r2, c2) = (r1 as usize, c1 as usize, r2 as usize, c2 as usize);

        let mut selected = String::new();
        for row in r1..=r2 {
            if row >= lines.len() {
                break;
            }
            let line = &lines[row];
            let chars: Vec<char> = line.chars().collect();
            let start_col = if row == r1 { c1 } else { 0 };
            let end_col = if row == r2 {
                (c2 + 1).min(chars.len())
            } else {
                chars.len()
            };
            if start_col < chars.len() {
                let slice: String = chars[start_col..end_col.min(chars.len())].iter().collect();
                selected.push_str(slice.trim_end());
            }
            if row < r2 {
                selected.push('\n');
            }
        }

        if selected.is_empty() {
            return;
        }

        // Use OSC 52 escape sequence to set the system clipboard.
        // This works directly through the terminal emulator (WezTerm, iTerm2, etc.)
        // without needing to spawn external processes in raw mode.
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(selected.as_bytes());
        let osc = format!("\x1b]52;c;{}\x07", encoded);
        let _ = std::io::Write::write_all(&mut std::io::stdout(), osc.as_bytes());
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }

    /// Snapshot all current sessions for restart/upgrade.
    pub(super) fn snapshot_sessions(&self) -> Vec<RestartEntry> {
        self.manager
            .sessions()
            .iter()
            .map(|s| RestartEntry {
                cwd: s.cwd.clone(),
                agent_kind: s.agent_kind,
                task_label: s.task_label.clone(),
                agent_session_id: s.agent_session_id.clone(),
            })
            .collect()
    }

    /// Kill all sessions (without recording history — they'll be resumed).
    pub(super) fn kill_all_for_restart(&mut self) {
        self.manager.kill_all_chatmux_sessions();
        self.manager.sessions_mut().clear();
        self.selected = None;
        self.terminal_content.clear();
        crate::session::state::remove();
    }

    /// Recreate sessions from a snapshot using resume commands.
    pub(super) fn recreate_from_snapshot(&mut self, snapshot: &[RestartEntry]) -> Result<()> {
        let width = self.terminal_area.width.saturating_sub(2);
        let height = self.terminal_area.height.saturating_sub(2);

        for entry in snapshot {
            let agent = self.registry.get(entry.agent_kind);
            let idx = self.manager.create_resume(
                &entry.cwd,
                agent,
                entry.agent_session_id.as_deref(),
                width,
                height,
            )?;
            if let Some(ref label) = entry.task_label {
                if let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                    session.task_label = Some(label.clone());
                }
            }
        }

        if !self.manager.is_empty() {
            self.selected = Some(0);
        }
        Ok(())
    }

    /// Execute restart-all: snapshot → kill → recreate.
    pub(super) fn do_restart_all(&mut self) -> Result<()> {
        let snapshot = self.snapshot_sessions();
        self.kill_all_for_restart();
        self.recreate_from_snapshot(&snapshot)?;
        Ok(())
    }

    /// Start upgrade + restart: snapshot → kill → run upgrade script.
    pub(super) fn do_upgrade_and_restart(&mut self) -> Result<()> {
        self.restart_snapshot = self.snapshot_sessions();
        self.kill_all_for_restart();

        let script = self.build_upgrade_script();
        let width = self.terminal_area.width.saturating_sub(2);
        let height = self.terminal_area.height.saturating_sub(2);

        self.manager.tmux().new_session_with_remain_on_exit(
            "upgrade",
            "/tmp",
            "sh",
            &["-c".into(), script],
            width,
            height,
        )?;
        self.upgrading = true;
        Ok(())
    }

    /// Called when the upgrade tmux session finishes.
    pub(super) fn finish_upgrade(&mut self) -> Result<()> {
        self.upgrading = false;
        let _ = self.manager.tmux().kill_session("upgrade");
        let snapshot = std::mem::take(&mut self.restart_snapshot);
        self.recreate_from_snapshot(&snapshot)?;
        Ok(())
    }

    /// Build a shell script that upgrades all agent kinds used in the snapshot.
    fn build_upgrade_script(&self) -> String {
        let mut kinds: HashSet<AgentKind> = self
            .restart_snapshot
            .iter()
            .map(|e| e.agent_kind)
            .collect();

        // If no sessions, upgrade all available agents.
        if kinds.is_empty() {
            for agent in self.registry.available() {
                kinds.insert(agent.kind());
            }
        }

        let mut commands = Vec::new();
        if kinds.contains(&AgentKind::ClaudeCode) {
            commands.push(self.config.upgrade.claude_code.clone());
        }
        if kinds.contains(&AgentKind::Codex) {
            commands.push(self.config.upgrade.codex.clone());
        }

        if commands.is_empty() {
            "echo 'No agents to upgrade'".into()
        } else {
            commands.join(" && ")
        }
    }

    /// Execute the confirmed action (called from input handler).
    pub(super) fn execute_confirmed_action(&mut self) -> Result<()> {
        let action = self.confirm_action.take();
        match action {
            Some(ConfirmAction::UpgradeAndRestart) => self.do_upgrade_and_restart(),
            Some(ConfirmAction::RestartAll) => self.do_restart_all(),
            None => Ok(()),
        }
    }

    /// Number of active sessions (for confirm dialog message).
    pub(super) fn session_count(&self) -> usize {
        self.manager.len()
    }

    pub fn cleanup(&mut self) {
        if self.upgrading {
            let _ = self.manager.tmux().kill_session("upgrade");
        }
        if self.detach_on_quit {
            self.manager.detach();
        } else {
            self.manager.cleanup();
        }
    }
}
