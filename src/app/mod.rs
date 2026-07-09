mod input;
mod input_keys;
mod input_mouse;
mod lifecycle;
mod render;
mod session_sync;
mod upgrade;

use crate::agent::{self, AgentKind, AgentRegistry};
use crate::config::{Config, ResolvedTheme};
use crate::session::model::SessionStatus;
use crate::session::state::HistoryEntry;
use crate::session::{SessionManager, SortMode};
use crate::tui::project_picker::{PickerMode, ProjectPicker, render_project_picker};
use crate::tui::render_startup_screen;
use crate::tui::sidebar::{
    SidebarParams, render_history_sidebar, render_project_list, render_sidebar,
    render_summary_bar, ProjectSummary,
};
use crate::tui::help::{HelpContext, render_confirm_overlay, render_help_overlay};
use crate::tui::terminal::{TerminalScroll, render_empty_terminal, render_terminal};
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
    Startup {
        existing_sessions: Vec<String>,
        /// True when tmux sessions are gone but saved state exists (e.g. after reboot).
        /// Restore will recreate tmux sessions using agent resume commands.
        cold_restore: bool,
    },
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
    DeleteSession { name: String },
    DeleteHistoryEntry { index: usize },
    OpenEditor { cwd: String },
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

/// Check if a key event is the prefix key (Ctrl+]).
/// Legacy terminals send Ctrl+] as Ctrl+5; modern (Kitty protocol) sends Ctrl+].
pub(super) fn is_prefix_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char(']') | KeyCode::Char('5'))
}

/// Sidebar UI state (view mode, selection, filter, scroll).
pub(super) struct SidebarState {
    pub(super) view: SidebarView,
    pub(super) sort_mode: SortMode,
    pub(super) filter_input: Option<String>,
    pub(super) rename_buf: Option<String>,
    pub(super) list_state: ListState,
    /// Project list view.
    pub(super) project_selected: usize,
    pub(super) project_list_state: ListState,
    /// History view.
    pub(super) show_history: bool,
    pub(super) history_entries: Vec<HistoryEntry>,
    pub(super) history_selected: usize,
    pub(super) history_list_state: ListState,
    /// Cached area for mouse click detection.
    pub(super) area: Rect,
}

impl Default for SidebarState {
    fn default() -> Self {
        Self {
            view: SidebarView::Sessions,
            sort_mode: SortMode::StatusPriority,
            filter_input: None,
            rename_buf: None,
            list_state: ListState::default(),
            project_selected: 0,
            project_list_state: ListState::default(),
            show_history: false,
            history_entries: Vec::new(),
            history_selected: 0,
            history_list_state: ListState::default(),
            area: Rect::default(),
        }
    }
}

/// Active search state within the terminal scrollback.
pub(super) struct SearchState {
    pub(super) pattern: String,
    /// Line offsets (from bottom) where matches were found.
    pub(super) matches: Vec<u16>,
    /// Current match index within `matches`.
    pub(super) current: usize,
}

/// Terminal pane state (content, scroll, selection).
#[derive(Default)]
pub(super) struct TerminalState {
    pub(super) content: String,
    /// Scroll offset: lines scrolled back from bottom (0 = live view).
    pub(super) scroll: u16,
    /// Cached history size captured when scrolling begins.
    pub(super) scroll_history: u16,
    /// Active text selection.
    pub(super) selection: Option<Selection>,
    /// Prefix mode active (Ctrl+] was pressed, waiting for next key).
    pub(super) prefix_active: bool,
    /// Cached area for pane sizing.
    pub(super) area: Rect,
    /// Search input buffer (Some while typing a search pattern).
    pub(super) search_input: Option<String>,
    /// Active search results.
    pub(super) search: Option<SearchState>,
}


pub struct App {
    pub(super) config: Config,
    pub(super) theme: ResolvedTheme,
    pub(super) registry: AgentRegistry,
    pub(super) manager: SessionManager,
    pub(super) selected: Option<String>,
    pub(super) focus: Focus,
    pub(super) mode: AppMode,
    pub(super) should_quit: bool,
    /// If true, save state on quit instead of killing sessions.
    pub(super) detach_on_quit: bool,
    pub(super) sidebar: SidebarState,
    pub(super) terminal: TerminalState,
    pub(super) picker: Option<ProjectPicker>,
    /// Cached project list (computed once at startup to avoid repeated I/O).
    pub(super) cached_projects: Vec<String>,
    /// Whether the help overlay is shown.
    pub(super) show_help: bool,
    /// Last time we checked JSONL files for status.
    pub(super) last_status_poll: Instant,
    /// Set of paths dirtied by the file watcher (shared with watcher thread).
    pub(super) watcher_dirty: Arc<Mutex<HashSet<PathBuf>>>,
    /// Keep the watcher alive (dropped on App drop).
    pub(super) _watcher: Option<notify::RecommendedWatcher>,
    /// Last time we checked the watcher dirty set.
    pub(super) last_watcher_check: Instant,
    /// Last time we auto-saved session state to disk.
    pub(super) last_auto_save: Instant,
    /// Last time we checked for hook events.
    pub(super) last_hook_check: Instant,
    /// Pending confirm action (U or R key).
    pub(super) confirm_action: Option<ConfirmAction>,
    /// Snapshot of sessions for restart/upgrade.
    pub(super) restart_snapshot: Vec<RestartEntry>,
    /// True while an upgrade tmux session is running.
    pub(super) upgrading: bool,
    /// Push notification of pane output (None → capture every frame).
    pub(super) pipe_watch: Option<crate::pipewatch::PipeWatch>,
    /// Session currently piped into the watch FIFO.
    pub(super) piped_session: Option<String>,
    /// Last time the selected pane was captured by the fallback timer.
    pub(super) last_capture_fallback: Instant,
    /// Scroll offset at the time of the last capture.
    pub(super) last_captured_scroll: u16,
}

impl App {
    pub fn new() -> Self {
        let config = Config::load();
        let theme = ResolvedTheme::from_config(&config.theme);
        let registry = AgentRegistry::new();
        let manager = SessionManager::new();
        let existing = manager.tmux().list_chatmux_sessions();
        let mode = if !existing.is_empty() {
            AppMode::Startup {
                existing_sessions: existing,
                cold_restore: false,
            }
        } else if let Some(saved) = crate::session::state::load() {
            if saved.sessions.is_empty() {
                AppMode::Normal
            } else {
                // tmux sessions are gone (e.g. after reboot) but saved state exists.
                let display: Vec<String> = saved
                    .sessions
                    .iter()
                    .map(|e| {
                        let agent = match e.agent_kind {
                            AgentKind::ClaudeCode => "claude",
                            AgentKind::Codex => "codex",
                        };
                        format!("{} ({})", e.project_name, agent)
                    })
                    .collect();
                AppMode::Startup {
                    existing_sessions: display,
                    cold_restore: true,
                }
            }
        } else {
            AppMode::Normal
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
            detach_on_quit: true,
            sidebar: SidebarState::default(),
            terminal: TerminalState::default(),
            picker: None,
            cached_projects,
            show_help: false,
            last_status_poll: Instant::now(),
            watcher_dirty: Arc::new(Mutex::new(HashSet::new())),
            _watcher: None,
            last_watcher_check: Instant::now(),
            last_auto_save: Instant::now(),
            last_hook_check: Instant::now(),
            confirm_action: None,
            restart_snapshot: Vec::new(),
            upgrading: false,
            pipe_watch: crate::pipewatch::PipeWatch::start(),
            piped_session: None,
            last_capture_fallback: Instant::now(),
            last_captured_scroll: 0,
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

    /// Resolve the selected session name to an index in the session list.
    pub(super) fn selected_index(&self) -> Option<usize> {
        let name = self.selected.as_ref()?;
        self.manager.sessions().iter().position(|s| &s.name == name)
    }

    /// Set the selected session by index (stores the session name).
    pub(super) fn select_by_index(&mut self, index: usize) {
        self.selected = self.manager.get(index).map(|s| s.name.clone());
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
                self.terminal.content = content;
            }
            if self.manager.tmux().is_pane_dead("upgrade") {
                let _ = self.finish_upgrade();
            }
            return;
        }

        // Resize tmux panes to match the terminal view area.
        // Skip sessions that are externally attached or already at the target size.
        let mut force_capture = false;
        let pane_width = self.terminal.area.width.saturating_sub(2);
        let pane_height = self.terminal.area.height.saturating_sub(2);
        if pane_width > 0 && pane_height > 0 {
            let target = (pane_width, pane_height);
            let needs_resize: Vec<usize> = (0..self.manager.len())
                .filter(|&i| {
                    self.manager.get(i).is_some_and(|s| {
                        !s.attached_externally && s.applied_size != Some(target)
                    })
                })
                .collect();
            let results: Vec<(usize, bool)> = needs_resize
                .into_iter()
                .map(|i| (i, self.manager.resize(i, pane_width, pane_height).is_ok()))
                .collect();
            let selected_idx = self.selected_index();
            for (i, ok) in results {
                if ok
                    && let Some(s) = self.manager.sessions_mut().get_mut(i)
                {
                    s.applied_size = Some(target);
                    // The selected pane re-wraps after a resize; re-capture it.
                    if selected_idx == Some(i) {
                        force_capture = true;
                    }
                }
            }
        }

        // Keep the output pipe bound to the selected session so new output
        // raises the dirty flag. On the fallback timer, re-issue the pipe:
        // a session recreated under the same name (restart/upgrade) silently
        // drops its pipe, and re-piping is idempotent.
        let fallback_due =
            self.last_capture_fallback.elapsed() >= self.config.polling.capture_fallback();
        if let Some(ref watch) = self.pipe_watch {
            if self.selected != self.piped_session {
                if let Some(old) = self.piped_session.take() {
                    self.manager.tmux().pipe_output_off(&old);
                }
                if let Some(ref name) = self.selected
                    && self.manager.tmux().pipe_output_to(name, watch.fifo_path()).is_ok()
                {
                    self.piped_session = Some(name.clone());
                }
                force_capture = true;
            } else if fallback_due
                && let Some(ref name) = self.selected
            {
                let _ = self.manager.tmux().pipe_output_to(name, watch.fifo_path());
            }
        }

        // Capture content for the selected session, but only when there is a
        // reason to: new output arrived (dirty), the scroll offset changed,
        // the pane was resized or reselected, or the fallback timer fired.
        // Without a pipe watch, capture every frame as before.
        if let Some(idx) = self.selected_index() {
            let dirty = self.pipe_watch.as_ref().is_none_or(|w| w.take_dirty());
            let scroll_changed = self.terminal.scroll != self.last_captured_scroll;
            if dirty || scroll_changed || force_capture || fallback_due {
                let result = if self.terminal.scroll > 0 {
                    self.manager
                        .capture_scroll(idx, self.terminal.scroll, pane_height)
                } else {
                    self.manager.capture(idx)
                };
                if let Ok(content) = result {
                    self.terminal.content = content;
                    self.last_captured_scroll = self.terminal.scroll;
                }
            }
        }
        if fallback_due {
            self.last_capture_fallback = Instant::now();
        }

        // Check for hook events from Claude Code (primary status source).
        // Also check for pending spool files from CLI-created sessions.
        if self.last_hook_check.elapsed() >= self.config.polling.hook_check() {
            self.last_hook_check = Instant::now();
            self.process_hook_events();
            if crate::spool::pending_dir().read_dir().is_ok_and(|mut d| d.next().is_some()) {
                self.discover_external_sessions();
                self.auto_sort();
            }
        }

        // Check watcher for dirty JSONL files and poll affected sessions.
        if self.last_watcher_check.elapsed() >= self.config.polling.watcher_debounce() {
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
        if self.last_status_poll.elapsed() >= self.config.polling.full_interval() {
            self.last_status_poll = Instant::now();
            self.discover_external_sessions();
            if let Some(name) = self.poll_session_statuses() {
                self.focus_session_by_name(&name);
            }
            self.auto_sort();
        }

        // Periodically auto-save state for crash recovery. Also saves when
        // empty: keeping a stale non-empty state file would offer a cold
        // restore of already-ended sessions after a crash.
        if self.last_auto_save.elapsed() >= self.config.polling.auto_save() {
            self.last_auto_save = Instant::now();
            self.manager.save_state();
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
                    has_input: false,
                    aggregate_status: SessionStatus::Read,
                    latest_activity_epoch: 0,
                }
            });
            entry.session_count += 1;
            if session.last_activity_epoch > entry.latest_activity_epoch {
                entry.latest_activity_epoch = session.last_activity_epoch;
            }
            match session.status {
                SessionStatus::InputRequired => {
                    entry.has_input = true;
                    entry.aggregate_status = SessionStatus::InputRequired;
                }
                SessionStatus::Replied => {
                    entry.has_replied = true;
                    if entry.aggregate_status != SessionStatus::InputRequired {
                        entry.aggregate_status = SessionStatus::Replied;
                    }
                }
                SessionStatus::Working => {
                    entry.has_working = true;
                    if !matches!(entry.aggregate_status, SessionStatus::InputRequired | SessionStatus::Replied) {
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
        let Some(ref filter) = self.sidebar.filter_input else {
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
        self.select_next_in(&visible);
    }

    pub(super) fn select_prev(&mut self) {
        let visible = self.visible_indices();
        self.select_prev_in(&visible);
    }

    /// Move selection to the next item within the given visible indices.
    pub(super) fn select_next_in(&mut self, visible: &[usize]) {
        if visible.is_empty() {
            return;
        }
        let old = self.selected.clone();
        let current_idx = self.selected_index();
        let new_idx = match current_idx {
            Some(current) => {
                if let Some(pos) = visible.iter().position(|&i| i == current) {
                    visible[(pos + 1).min(visible.len() - 1)]
                } else {
                    visible[0]
                }
            }
            None => visible[0],
        };
        self.select_by_index(new_idx);
        if self.selected != old {
            self.terminal.scroll = 0;
        }
    }

    /// Move selection to the previous item within the given visible indices.
    pub(super) fn select_prev_in(&mut self, visible: &[usize]) {
        if visible.is_empty() {
            return;
        }
        let old = self.selected.clone();
        let current_idx = self.selected_index();
        let new_idx = match current_idx {
            Some(current) => {
                if let Some(pos) = visible.iter().position(|&i| i == current) {
                    visible[pos.saturating_sub(1)]
                } else {
                    visible[0]
                }
            }
            None => visible[0],
        };
        self.select_by_index(new_idx);
        if self.selected != old {
            self.terminal.scroll = 0;
        }
    }

    /// Handle a click on a session list (used by both Sessions and ProjectSessions views).
    pub(super) fn click_session_list(&mut self, content_y: usize, visible: &[usize]) -> Result<()> {
        let offset = self.sidebar.list_state.offset();
        let has_filter = self.sidebar.filter_input.is_some();
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
                    self.select_by_index(visible[vis_idx]);
                    self.terminal.scroll = 0;
                    self.focus = Focus::Sidebar;
                }
                return Ok(());
            }
            y_accum += height;
        }
        Ok(())
    }

    /// If the selected session is Replied, mark it as Read.
    pub(super) fn mark_selected_as_read(&mut self) {
        if let Some(idx) = self.selected_index()
            && let Some(session) = self.manager.sessions_mut().get_mut(idx)
                && session.status == crate::session::SessionStatus::Replied {
                    session.status = crate::session::SessionStatus::Read;
                }
    }

    /// Ensure the selected session is within the visible (filtered) list.
    pub(super) fn ensure_selected_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() {
            return;
        }
        if let Some(sel_idx) = self.selected_index()
            && !visible.contains(&sel_idx) {
                self.select_by_index(visible[0]);
            }
    }

    /// When the selected session transitions to Working while the user is
    /// viewing it in Terminal, switch back to Sidebar so the user can watch
    /// other sessions while the agent works.
    fn focus_session_by_name(&mut self, name: &str) {
        if self.focus != Focus::Terminal {
            return;
        }
        if self.selected.as_deref() == Some(name) {
            self.focus = Focus::Sidebar;
        }
    }

    /// Extract plain text lines from the terminal content (ANSI stripped).
    pub(super) fn plain_lines(&self) -> Vec<String> {
        use ansi_to_tui::IntoText;
        let text = self
            .terminal.content
            .as_bytes()
            .into_text()
            .unwrap_or_else(|_| ratatui::text::Text::raw(&self.terminal.content));
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

    /// Number of active sessions (for confirm dialog message).
    pub(super) fn session_count(&self) -> usize {
        self.manager.len()
    }

    pub fn cleanup(&mut self) {
        if self.upgrading {
            let _ = self.manager.tmux().kill_session("upgrade");
        }
        // Stop piping pane output; a dangling pipe would fill the FIFO
        // once nothing drains it.
        if let Some(old) = self.piped_session.take() {
            self.manager.tmux().pipe_output_off(&old);
        }
        // If we quit from the Startup screen without choosing restore or
        // new, leave tmux sessions and saved state untouched so the user
        // can restore next time.
        if matches!(self.mode, AppMode::Startup { .. }) {
            return;
        }
        if self.detach_on_quit {
            self.manager.detach();
        } else {
            self.manager.cleanup();
        }
    }

    /// Save session state without killing sessions (for crash/panic recovery).
    pub fn save_state_for_crash_recovery(&self) {
        // Skip while on the startup screen: the manager is still empty there
        // and saving would clobber the state we may want to restore.
        if matches!(self.mode, AppMode::Startup { .. }) {
            return;
        }
        self.manager.save_state();
    }
}
