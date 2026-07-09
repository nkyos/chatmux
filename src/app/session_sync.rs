use super::*;

impl App {
    /// Process hook events from Claude Code for all sessions.
    /// This is the primary status detection mechanism for Claude sessions.
    pub(super) fn process_hook_events(&mut self) {
        let notifications_enabled = self.config.notifications.enabled;
        let notify_statuses = self.config.notifications.statuses.clone();
        let sound = self.config.notifications.sound.clone();
        let mut any_changed = false;

        for session in self.manager.sessions_mut() {
            let events = crate::hooks::drain_events(&session.name);
            if events.is_empty() {
                continue;
            }

            for event in events {
                let old_status = session.status.clone();
                match event.hook_event_name.as_str() {
                    "SessionStart" => {
                        if let Some(new_id) = &event.session_id
                            && session.agent_session_id.as_ref() != Some(new_id)
                        {
                            session.agent_session_id = Some(new_id.clone());
                            let agent = self.registry.get(session.agent_kind);
                            session.jsonl_path =
                                agent.session_file_for(&session.cwd, new_id);
                            session.jsonl_stamp = None;
                        }
                    }
                    "UserPromptSubmit" => {
                        session.status = SessionStatus::Working;
                        session.touch_activity();
                        if let Some(prompt) = &event.prompt {
                            session.last_prompt = Some(prompt.clone());
                        }
                    }
                    "Stop" => {
                        session.status = SessionStatus::Replied;
                        session.touch_activity();
                        if let Some(reply) = &event.last_assistant_message {
                            session.last_reply = Some(reply.clone());
                        }
                    }
                    "Notification" => {
                        session.status = SessionStatus::InputRequired;
                        session.touch_activity();
                    }
                    _ => {}
                }

                if session.status != old_status {
                    any_changed = true;
                    if notifications_enabled
                        && notify_statuses
                            .contains(&session.status.name().to_string())
                    {
                        crate::notify::notify_status(
                            &session.project_name,
                            &format!(
                                "{} {}",
                                session.agent_kind.label(),
                                session.status.name()
                            ),
                            &sound,
                            session.last_reply.as_deref(),
                        );
                    }
                }
            }
        }

        if any_changed {
            self.auto_sort();
        }
    }

    /// Discover chatmux tmux sessions created externally (e.g. via `chatmux claude`).
    /// Also removes sessions whose tmux session has died, and picks up
    /// spool metadata from CLI-created sessions.
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
            for session in self.manager.sessions() {
                if !live.contains(&session.name) {
                    crate::session::state::append_history(&session.to_history_entry());
                }
            }

            self.manager.sessions_mut().retain(|s| live.contains(&s.name));

            // If the selected session was removed, pick the first available.
            if self.selected_index().is_none() && !self.manager.is_empty() {
                self.select_by_index(0);
            }
        }

        let tracked: std::collections::HashSet<String> = self
            .manager
            .sessions()
            .iter()
            .map(|s| s.name.clone())
            .collect();

        // Read spool files for CLI-created sessions.
        let pending = crate::spool::list_pending();

        // Collect info for new sessions first (avoids borrow conflicts).
        let new_sessions: Vec<_> = live.iter()
            .filter(|name| !tracked.contains(*name))
            .map(|name| {
                // Check spool file for metadata first.
                if let Some((_, spool)) = pending.iter().find(|(n, _)| n == name) {
                    let has_client = self.manager.tmux().has_attached_client(name);
                    return (
                        name.clone(),
                        spool.cwd.clone(),
                        spool.agent_kind,
                        has_client,
                        Some(spool),
                    );
                }

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
                (name.clone(), cwd, agent_kind, has_client, None)
            })
            .collect();

        for (name, cwd, agent_kind, has_client, spool) in new_sessions {
            let mut session =
                crate::session::Session::new(name.clone(), cwd.clone(), agent_kind);
            session.attached_externally = has_client;

            // Apply spool metadata and remove the spool file.
            if let Some(spool) = spool {
                session.agent_session_id = spool.agent_session_id.clone();
                session.jsonl_path = spool.session_file.as_ref().map(std::path::PathBuf::from);
                session.task_label = spool.task_label.clone();
                if spool.branch.is_some() {
                    session.branch = spool.branch.clone();
                }
                crate::spool::remove_spool(&name);
            }

            self.manager.sessions_mut().push(session);

            if let Some(num) = name.strip_prefix('s').and_then(|n| n.parse::<usize>().ok()) {
                self.manager.ensure_next_id(num + 1);
            }
        }

        // Clean up stale spool files (exec failed, session never appeared).
        crate::spool::cleanup_stale(&live, 3600);

        // Update attached_externally flag for all tracked sessions.
        let attach_status: Vec<_> = self.manager.sessions()
            .iter()
            .map(|s| self.manager.tmux().has_attached_client(&s.name))
            .collect();
        for (session, attached) in self.manager.sessions_mut().iter_mut().zip(attach_status) {
            if session.attached_externally && !attached {
                session.applied_size = None;
            }
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
    pub(super) fn detect_agent_kind(&self, name: &str) -> Option<AgentKind> {
        self.manager.tmux().detect_agent_kind(name)
    }

    /// Check session files for each session and update their status via agent adapters.
    /// Returns the name of a session that just transitioned to Working (user sent a prompt).
    pub(super) fn poll_session_statuses(&mut self) -> Option<String> {
        let notifications_enabled = self.config.notifications.enabled;
        let notify_statuses = self.config.notifications.statuses.clone();
        let sound = self.config.notifications.sound.clone();

        // Pre-collect pane commands so we can check if agents are still running
        // without borrowing self.manager immutably inside the mutable loop.
        let pane_commands: std::collections::HashMap<String, String> = self
            .manager
            .sessions()
            .iter()
            .filter_map(|s| {
                self.manager
                    .tmux()
                    .get_pane_command(&s.name)
                    .map(|cmd| (s.name.clone(), cmd))
            })
            .collect();

        // Build the set of assigned JSONL paths to prevent fallback collisions.
        let assigned_paths: HashSet<PathBuf> = self
            .manager
            .sessions()
            .iter()
            .filter_map(|s| s.jsonl_path.clone())
            .collect();

        let registry = &self.registry;
        let mut became_working: Option<String> = None;

        for session in self.manager.sessions_mut() {
            let agent_adapter = registry.get(session.agent_kind);

            // Resolve the JSONL file path.
            // Sessions with a deterministic agent_session_id skip heuristic
            // resolution: the JSONL file may not exist yet (created on first
            // message), so we wait instead of falling back to guessing.
            {
                let has_deterministic_path = session.agent_session_id.is_some()
                    && session.jsonl_path.is_some();

                let needs_resolve = !has_deterministic_path
                    && (session.jsonl_path.is_none()
                        || session.jsonl_path.as_ref().is_some_and(|p| !p.exists()));

                if needs_resolve {
                    session.jsonl_stamp = None;
                    // Exclude paths assigned to other sessions from fallback.
                    let exclude: HashSet<PathBuf> = assigned_paths.iter()
                        .filter(|p| session.jsonl_path.as_ref() != Some(p))
                        .cloned()
                        .collect();
                    let found = find_best_session_file(
                        agent_adapter,
                        &session.cwd,
                        session.agent_session_id.as_deref(),
                        &exclude,
                    );
                    if found.is_none()
                        || session.agent_session_id.as_ref().is_some_and(|id| {
                            found.as_ref().is_some_and(|p| {
                                p.file_stem()
                                    .and_then(|s| s.to_str())
                                    .is_none_or(|s| s != id.as_str())
                            })
                        })
                    {
                        session.agent_session_id = None;
                    }
                    session.jsonl_path = found;
                }
            }

            // Refresh git branch (may change during session).
            // Placed before JSONL guards so idle/completed sessions still get updated.
            session.refresh_branch();

            // Extract agent session ID when JSONL is first resolved.
            if session.agent_session_id.is_none()
                && let Some(ref path) = session.jsonl_path {
                    session.agent_session_id = agent_adapter.extract_session_id(path);
                }

            let Some(ref jsonl_path) = session.jsonl_path else {
                continue;
            };

            // Only re-read if the file has changed (mtime or size).
            let current_stamp = agent::file_stamp(jsonl_path);
            let file_changed = current_stamp != session.jsonl_stamp || session.jsonl_stamp.is_none();
            if file_changed {
                session.jsonl_stamp = current_stamp;
            }

            if !file_changed {
                let agent_exited = pane_commands.get(&session.name)
                    .is_some_and(|cmd| matches!(cmd.as_str(), "zsh" | "bash" | "fish" | "sh" | "nu" | "dash"));

                // Agent exited: if the pane fell back to a shell while status
                // is Working, the agent exited without writing end_turn.
                if matches!(session.status, SessionStatus::Working | SessionStatus::InputRequired) && agent_exited {
                    session.status = SessionStatus::Replied;
                    session.touch_activity();
                    if notifications_enabled
                        && notify_statuses.contains(&SessionStatus::Replied.name().to_string())
                    {
                        crate::notify::notify_status(
                            &session.project_name,
                            &format!("{} {}", session.agent_kind.label(), SessionStatus::Replied.name()),
                            &sound,
                            session.last_reply.as_deref(),
                        );
                    }
                }


                continue;
            }

            // Detect status from the session file.
            if let Some(detected) = agent_adapter.detect_status(jsonl_path)
                && let Some(name) = apply_detected_status(
                    session,
                    &detected,
                    notifications_enabled,
                    &notify_statuses,
                    &sound,
                ) {
                    became_working = Some(name);
                }
        }

        became_working
    }

    /// Poll only sessions whose JSONL file was dirtied by the watcher.
    /// Also detects unassigned dirty files that could belong to tracked sessions
    /// (e.g. after /clear) and triggers an immediate full poll when found.
    /// Returns the name of a session that just transitioned to Working (user sent a prompt).
    pub(super) fn poll_dirty_sessions(&mut self, dirty: &HashSet<PathBuf>) -> Option<String> {
        let notifications_enabled = self.config.notifications.enabled;
        let notify_statuses = self.config.notifications.statuses.clone();
        let sound = self.config.notifications.sound.clone();
        let registry = &self.registry;
        let mut became_working: Option<String> = None;

        let assigned_paths: HashSet<PathBuf> = self
            .manager
            .sessions()
            .iter()
            .filter_map(|s| s.jsonl_path.clone())
            .collect();

        // Only trigger full poll for unassigned files that could belong to tracked sessions.
        let tracked_encoded_cwds: HashSet<String> = self
            .manager
            .sessions()
            .iter()
            .filter(|s| s.agent_kind == AgentKind::ClaudeCode)
            .map(|s| crate::agent::encode_project_path(&s.cwd))
            .collect();
        let tracked_codex_cwds: HashSet<String> = self
            .manager
            .sessions()
            .iter()
            .filter(|s| s.agent_kind == AgentKind::Codex)
            .map(|s| s.cwd.clone())
            .collect();

        let has_unassigned = dirty.iter().any(|p| {
            if assigned_paths.contains(p) {
                return false;
            }
            // Claude: parent dir name must match an encoded cwd of a tracked session.
            if let Some(parent) = p.parent()
                && let Some(dir_name) = parent.file_name().and_then(|n| n.to_str())
                && tracked_encoded_cwds.contains(dir_name)
            {
                return true;
            }
            // Codex: only consider if we have any tracked Codex sessions.
            if !tracked_codex_cwds.is_empty()
                && p.to_str().is_some_and(|s| s.contains(".codex/sessions/"))
            {
                return true;
            }
            false
        });

        for session in self.manager.sessions_mut() {
            let Some(ref jsonl_path) = session.jsonl_path else {
                continue;
            };
            if !dirty.contains(jsonl_path) {
                continue;
            }

            let agent_adapter = registry.get(session.agent_kind);
            session.jsonl_stamp = agent::file_stamp(jsonl_path);

            if let Some(detected) = agent_adapter.detect_status(jsonl_path)
                && let Some(name) = apply_detected_status(
                    session,
                    &detected,
                    notifications_enabled,
                    &notify_statuses,
                    &sound,
                ) {
                    became_working = Some(name);
                }
        }

        // If dirty files include paths not assigned to any session,
        // trigger a full poll immediately to reassign (handles /clear).
        if has_unassigned {
            self.last_status_poll = Instant::now() - STATUS_POLL_INTERVAL;
        }

        became_working
    }

    /// Apply the current sort mode to the session list.
    pub(super) fn auto_sort(&mut self) {
        if self.manager.is_empty() {
            return;
        }
        match self.sidebar.sort_mode {
            SortMode::StatusPriority => {
                self.manager.sort_by_priority();
            }
            SortMode::LastActivity => {
                self.manager.sort_by_activity();
            }
            SortMode::Manual => {}
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

        if let Ok(home) = std::env::var("HOME") {
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

/// Find the best session file for a chatmux session.
/// This is a minimal fallback for codex and picker sessions that lack
/// a deterministic session ID. Claude sessions with --session-id use
/// hooks-based detection and never reach this path.
///
/// Resolution order:
/// 1. Direct match by `agent_session_id` (filename = `{id}.jsonl`).
/// 2. Fallback: most recently modified file (excluding paths in `exclude`).
fn find_best_session_file(
    agent: &dyn crate::agent::Agent,
    cwd: &str,
    agent_session_id: Option<&str>,
    exclude: &HashSet<PathBuf>,
) -> Option<std::path::PathBuf> {
    let all_files = agent.list_session_files(cwd);

    // 1. Direct match by agent session ID (not subject to exclusion).
    if let Some(id) = agent_session_id
        && let Some(path) = all_files.iter().find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s == id)
        })
    {
        return Some(path.clone());
    }

    if all_files.is_empty() {
        return None;
    }

    // 2. Fallback: most recently modified file, excluding assigned paths.
    all_files
        .into_iter()
        .filter(|p| !exclude.contains(p))
        .filter_map(|p| {
            let mtime = p.metadata().ok()?.modified().ok()?;
            Some((p, mtime))
        })
        .max_by_key(|(_, t)| *t)
        .map(|(p, _)| p)
}

/// Apply a detected status to a session, sending notifications if configured.
/// Returns the session name if it just transitioned to Working.
fn apply_detected_status(
    session: &mut crate::session::Session,
    detected: &crate::agent::DetectedStatus,
    notifications_enabled: bool,
    notify_statuses: &[String],
    sound: &str,
) -> Option<String> {
    let old_status = session.status.clone();
    // Skip Read→Replied regression: the user already saw this reply.
    let is_read_regression = old_status == SessionStatus::Read
        && detected.status == SessionStatus::Replied;

    let mut became_working = None;

    if old_status != detected.status && !is_read_regression {
        session.status = detected.status.clone();
        session.touch_activity();

        if detected.status == SessionStatus::Working {
            became_working = Some(session.name.clone());
        }

        if notifications_enabled
            && notify_statuses.contains(&detected.status.name().to_string())
        {
            crate::notify::notify_status(
                &session.project_name,
                &format!("{} {}", session.agent_kind.label(), detected.status.name()),
                sound,
                detected.last_reply.as_deref(),
            );
        }
    }

    if detected.last_prompt.is_some() && detected.last_prompt != session.last_prompt {
        session.last_prompt = detected.last_prompt.clone();
    }
    if detected.last_reply.is_some() && detected.last_reply != session.last_reply {
        session.last_reply = detected.last_reply.clone();
    }

    became_working
}

#[cfg(test)]
mod tests {
    use super::*;
    struct MockAgent {
        files: Vec<std::path::PathBuf>,
    }

    impl crate::agent::Agent for MockAgent {
        fn kind(&self) -> AgentKind {
            AgentKind::ClaudeCode
        }
        fn command(&self) -> &str {
            "mock"
        }
        fn list_session_files(&self, _cwd: &str) -> Vec<std::path::PathBuf> {
            self.files.clone()
        }
        fn detect_status(
            &self,
            _session_file: &std::path::Path,
        ) -> Option<crate::agent::DetectedStatus> {
            None
        }
        fn discover_projects(&self) -> Vec<String> {
            vec![]
        }
    }

    #[test]
    fn find_by_session_id_direct_match() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("abc-123.jsonl");
        let other = dir.path().join("xyz-456.jsonl");
        std::fs::File::create(&target).unwrap();
        std::fs::File::create(&other).unwrap();

        let agent = MockAgent {
            files: vec![target.clone(), other],
        };

        let empty = HashSet::new();
        let result = find_best_session_file(&agent, "/tmp/test", Some("abc-123"), &empty);
        assert_eq!(result, Some(target));
    }

    #[test]
    fn find_returns_most_recent_without_id() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.jsonl");
        let new = dir.path().join("new.jsonl");
        std::fs::File::create(&old).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::File::create(&new).unwrap();

        let agent = MockAgent {
            files: vec![old, new.clone()],
        };

        let empty = HashSet::new();
        let result = find_best_session_file(&agent, "/tmp/test", None, &empty);
        assert_eq!(result, Some(new));
    }

    #[test]
    fn find_returns_none_when_no_files() {
        let agent = MockAgent { files: vec![] };
        let empty = HashSet::new();
        let result = find_best_session_file(&agent, "/tmp/test", None, &empty);
        assert!(result.is_none());
    }

    #[test]
    fn find_none_session_id_uses_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("aaa.jsonl");
        let f2 = dir.path().join("bbb.jsonl");
        std::fs::File::create(&f1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::File::create(&f2).unwrap();

        let agent = MockAgent {
            files: vec![f1, f2.clone()],
        };

        let empty = HashSet::new();
        let result = find_best_session_file(&agent, "/tmp/test", None, &empty);
        assert_eq!(result, Some(f2));
    }

    #[test]
    fn find_excludes_assigned_paths() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("aaa.jsonl");
        let f2 = dir.path().join("bbb.jsonl");
        std::fs::File::create(&f1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::File::create(&f2).unwrap();

        let agent = MockAgent {
            files: vec![f1.clone(), f2.clone()],
        };

        // f2 is the newest but is excluded — should fall back to f1.
        let mut exclude = HashSet::new();
        exclude.insert(f2);
        let result = find_best_session_file(&agent, "/tmp/test", None, &exclude);
        assert_eq!(result, Some(f1));
    }
}
