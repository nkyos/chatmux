use super::*;

impl App {
    /// Discover chatmux tmux sessions created externally (e.g. via `chatmux claude`).
    /// Also removes sessions whose tmux session has died.
    pub(super) fn discover_external_sessions(&mut self) {
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
    pub(super) fn agent_kind_from_state(&self, name: &str) -> Option<AgentKind> {
        let saved = crate::session::state::load()?;
        saved.sessions.iter()
            .find(|e| e.name == name)
            .map(|e| e.agent_kind)
    }

    /// Detect agent kind by checking the process tree inside the tmux pane.
    /// `pane_current_command` may return the parent shell (e.g. "fish"),
    /// so we also check the pane's full command line via pane_start_command.
    pub(super) fn detect_agent_kind(&self, name: &str) -> Option<AgentKind> {
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
    /// Returns the name of a session that just transitioned to Working (user sent a prompt).
    pub(super) fn poll_session_statuses(&mut self) -> Option<String> {
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
        let mut became_working: Option<String> = None;

        for session in self.manager.sessions_mut() {
            let agent_adapter = registry.get(session.agent_kind);

            // Resolve the session file path.
            // Re-resolve if not yet found or if the file has been deleted.
            if session.jsonl_path.is_none()
                || session
                    .jsonl_path
                    .as_ref()
                    .is_some_and(|p| !p.exists())
            {
                session.jsonl_stamp = None;
                let mut exclude = session.pre_existing_files.clone();
                exclude.extend(assigned.iter().cloned());
                session.jsonl_path = agent_adapter.find_session_file(&session.cwd, &exclude);
                // Track newly assigned path so subsequent sessions won't claim the same file.
                if let Some(ref path) = session.jsonl_path {
                    assigned.push(path.clone());
                }
            }

            // Extract agent session ID when JSONL is first resolved.
            if session.agent_session_id.is_none() {
                if let Some(ref path) = session.jsonl_path {
                    session.agent_session_id = agent_adapter.extract_session_id(path);
                }
            }

            let Some(ref jsonl_path) = session.jsonl_path else {
                continue;
            };

            // Only re-read if the file has changed (mtime or size).
            let current_stamp = agent::file_stamp(jsonl_path);
            if current_stamp == session.jsonl_stamp && session.jsonl_stamp.is_some() {
                continue;
            }
            session.jsonl_stamp = current_stamp;

            // Detect status from the session file.
            if let Some(detected) = agent_adapter.detect_status(jsonl_path) {
                let old_status = session.status.clone();
                if old_status != detected.status {
                    session.status = detected.status.clone();
                    session.touch_activity();

                    // Track if this session just started working (user sent a prompt).
                    if detected.status == SessionStatus::Working {
                        became_working = Some(session.name.clone());
                    }

                    // Send notification if this status is in the notify list.
                    if notifications_enabled
                        && notify_statuses.contains(&detected.status.name().to_string())
                    {
                        crate::notify::notify_status(
                            &session.project_name,
                            &format!("{} {}", session.agent_kind.label(), detected.status.name()),
                            &sound,
                            detected.last_reply.as_deref(),
                        );
                    }
                }
                // Update last prompt if changed.
                if detected.last_prompt.is_some() && detected.last_prompt != session.last_prompt {
                    session.last_prompt = detected.last_prompt;
                }
                // Update last reply if changed.
                if detected.last_reply.is_some() && detected.last_reply != session.last_reply {
                    session.last_reply = detected.last_reply;
                }
            }

            // Refresh git branch (may change during session).
            session.refresh_branch();
        }

        became_working
    }

    /// Poll only sessions whose JSONL file was dirtied by the watcher.
    /// Returns the name of a session that just transitioned to Working (user sent a prompt).
    pub(super) fn poll_dirty_sessions(&mut self, dirty: &HashSet<PathBuf>) -> Option<String> {
        let notifications_enabled = self.config.notifications.enabled;
        let notify_statuses = self.config.notifications.statuses.clone();
        let sound = self.config.notifications.sound.clone();
        let registry = &self.registry;
        let mut became_working: Option<String> = None;

        for session in self.manager.sessions_mut() {
            let Some(ref jsonl_path) = session.jsonl_path else {
                continue;
            };
            if !dirty.contains(jsonl_path) {
                continue;
            }

            let agent_adapter = registry.get(session.agent_kind);

            // Update the stamp since the file changed.
            session.jsonl_stamp = agent::file_stamp(jsonl_path);

            if let Some(detected) = agent_adapter.detect_status(jsonl_path) {
                let old_status = session.status.clone();
                if old_status != detected.status {
                    session.status = detected.status.clone();
                    session.touch_activity();

                    // Track if this session just started working (user sent a prompt).
                    if detected.status == SessionStatus::Working {
                        became_working = Some(session.name.clone());
                    }

                    if notifications_enabled
                        && notify_statuses.contains(&detected.status.name().to_string())
                    {
                        crate::notify::notify_status(
                            &session.project_name,
                            &format!("{} {}", session.agent_kind.label(), detected.status.name()),
                            &sound,
                            detected.last_reply.as_deref(),
                        );
                    }
                }
                if detected.last_prompt.is_some() && detected.last_prompt != session.last_prompt {
                    session.last_prompt = detected.last_prompt;
                }
                if detected.last_reply.is_some() && detected.last_reply != session.last_reply {
                    session.last_reply = detected.last_reply;
                }
            }
        }

        became_working
    }

    /// Apply the current sort mode to the session list.
    pub(super) fn auto_sort(&mut self) {
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

    /// Start a filesystem watcher on JSONL directories.
    pub(super) fn start_watcher(&mut self) {
        use notify::{RecursiveMode, Watcher, event::EventKind};

        let dirty = Arc::clone(&self.watcher_dirty);
        let watcher = notify::RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) => {
                            if let Ok(mut set) = dirty.lock() {
                                for path in event.paths {
                                    if path.extension().is_some_and(|e| e == "jsonl") {
                                        set.insert(path);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            },
            notify::Config::default(),
        );

        let Ok(mut watcher) = watcher else { return };

        if let Some(home) = std::env::var("HOME").ok() {
            let claude_dir = std::path::Path::new(&home).join(".claude/projects");
            if claude_dir.is_dir() {
                let _ = watcher.watch(&claude_dir, RecursiveMode::Recursive);
            }
            let codex_dir = std::path::Path::new(&home).join(".codex/sessions");
            if codex_dir.is_dir() {
                let _ = watcher.watch(&codex_dir, RecursiveMode::Recursive);
            }
        }

        self._watcher = Some(watcher);
    }
}
