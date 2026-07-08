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
            session.created_epoch = created_epoch;

            // For externally discovered sessions, compute pre_existing_files
            // using the tmux session's creation time. Files created before the
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
                            .and_then(|m| m.created().or_else(|_| m.modified()).ok())
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

        let registry = &self.registry;
        let mut became_working: Option<String> = None;

        for session in self.manager.sessions_mut() {
            let agent_adapter = registry.get(session.agent_kind);

            // Resolve the session file path.
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
                    let mut exclude = session.pre_existing_files.clone();
                    for p in &assigned {
                        exclude.push(p.clone());
                    }
                    let found = find_best_session_file(
                        agent_adapter,
                        &session.cwd,
                        &exclude,
                        session.created_epoch,
                        session.agent_session_id.as_deref(),
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
                    if let Some(ref path) = found
                        && !assigned.contains(path) {
                            assigned.push(path.clone());
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
                let agent_running = pane_commands.get(&session.name)
                    .is_some_and(|cmd| matches!(cmd.as_str(), "claude" | "codex" | "node"));

                // /clear detection: if the agent is still running but its
                // JSONL file stopped changing, look for a newer file that is
                // actively being written to.  Only switch if the candidate was
                // modified very recently (within 3s), which means it's the file
                // the agent is currently writing — not a leftover from another
                // session.  This prevents false switches for idle sessions.
                if agent_running && session.agent_session_id.is_some() {
                    let now = std::time::SystemTime::now();
                    let recency_threshold = std::time::Duration::from_secs(3);
                    let own_mtime = session.jsonl_stamp.and_then(|s| s.modified);

                    let all_files = agent_adapter.list_session_files(&session.cwd);
                    let newest = all_files
                        .into_iter()
                        .filter(|p| !session.pre_existing_files.contains(p))
                        .filter(|p| !assigned.contains(p))
                        .filter(|p| session.jsonl_path.as_ref() != Some(p))
                        .filter_map(|p| {
                            let mtime = p.metadata().ok()?.modified().ok()?;
                            // Must be newer than our current file.
                            if own_mtime.is_some_and(|own| mtime <= own) {
                                return None;
                            }
                            // Must have been written to recently (actively in use).
                            if now.duration_since(mtime).ok()? > recency_threshold {
                                return None;
                            }
                            Some((p, mtime))
                        })
                        .max_by_key(|(_, mtime)| *mtime)
                        .map(|(p, _)| p);

                    if let Some(new_path) = newest {
                        assigned.push(new_path.clone());
                        session.jsonl_path = Some(new_path);
                        session.jsonl_stamp = None;
                        session.agent_session_id = None;
                        continue;
                    }
                }

                // Agent exited: if the pane fell back to a shell while status
                // is Working, the agent exited without writing end_turn.
                if matches!(session.status, SessionStatus::Working | SessionStatus::InputRequired) && !agent_running {
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
    /// Also detects unassigned dirty files (e.g. after /clear) and triggers
    /// an immediate full poll when found.
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

        let has_unassigned = dirty.iter().any(|p| !assigned_paths.contains(p));

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
        // Remember selected session name to preserve selection after sort.
        let selected_name = self
            .selected
            .and_then(|i| self.manager.get(i))
            .map(|s| s.name.clone());

        match self.sidebar.sort_mode {
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
///
/// Resolution order:
/// 1. Direct match by `agent_session_id` (filename = `{id}.jsonl`) — most reliable.
/// 2. Birthtime match: file whose creation time is closest to `created_epoch`.
/// 3. Fallback: oldest file by creation time.
fn find_best_session_file(
    agent: &dyn crate::agent::Agent,
    cwd: &str,
    exclude: &[std::path::PathBuf],
    created_epoch: Option<u64>,
    agent_session_id: Option<&str>,
) -> Option<std::path::PathBuf> {
    let all_files = agent.list_session_files(cwd);

    // 1. Direct match by agent session ID (most reliable).
    if let Some(id) = agent_session_id
        && let Some(path) = all_files.iter().find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s == id)
        })
    {
        return Some(path.clone());
    }

    let candidates: Vec<std::path::PathBuf> = all_files
        .into_iter()
        .filter(|p| !exclude.contains(p))
        .collect();

    if candidates.is_empty() {
        return None;
    }

    // 2. Birthtime match: prefer the file whose birthtime is closest to
    //    (and after) the session's creation time.
    if let Some(epoch) = created_epoch {
        let session_time =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch);
        let mut best: Option<(std::path::PathBuf, std::time::Duration)> = None;
        for p in &candidates {
            if let Ok(meta) = p.metadata() {
                let file_time = meta.created().or_else(|_| meta.modified()).ok();
                if let Some(t) = file_time
                    && let Ok(diff) = t.duration_since(session_time)
                        && best.as_ref().is_none_or(|(_, d)| diff < *d) {
                            best = Some((p.clone(), diff));
                        }
            }
        }
        if let Some((path, _)) = best {
            return Some(path);
        }
    }

    // 3. Fallback: pick the oldest file by creation time.
    let mut with_time: Vec<(std::path::PathBuf, std::time::SystemTime)> = candidates
        .into_iter()
        .filter_map(|p| {
            let meta = p.metadata().ok()?;
            let t = meta.created().or_else(|_| meta.modified()).ok()?;
            Some((p, t))
        })
        .collect();
    with_time.sort_by_key(|(_, t)| *t);
    with_time.into_iter().next().map(|(p, _)| p)
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

        let result = find_best_session_file(&agent, "/tmp/test", &[], None, Some("abc-123"));
        assert_eq!(result, Some(target));
    }

    #[test]
    fn find_excludes_pre_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.jsonl");
        let new = dir.path().join("new.jsonl");
        std::fs::File::create(&old).unwrap();
        // Small delay to ensure different timestamps.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::File::create(&new).unwrap();

        let agent = MockAgent {
            files: vec![old.clone(), new.clone()],
        };

        let result = find_best_session_file(&agent, "/tmp/test", &[old], None, None);
        assert_eq!(result, Some(new));
    }

    #[test]
    fn find_returns_none_when_all_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("only.jsonl");
        std::fs::File::create(&f).unwrap();

        let agent = MockAgent {
            files: vec![f.clone()],
        };

        let result = find_best_session_file(&agent, "/tmp/test", &[f], None, None);
        assert!(result.is_none());
    }

    #[test]
    fn find_returns_none_when_no_files() {
        let agent = MockAgent { files: vec![] };
        let result = find_best_session_file(&agent, "/tmp/test", &[], None, None);
        assert!(result.is_none());
    }

    #[test]
    fn find_id_match_ignores_exclude_list() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("my-id.jsonl");
        std::fs::File::create(&target).unwrap();

        let agent = MockAgent {
            files: vec![target.clone()],
        };

        // Even if the file is in the exclude list, ID match should still find it.
        let result =
            find_best_session_file(&agent, "/tmp/test", &[target.clone()], None, Some("my-id"));
        assert_eq!(result, Some(target));
    }

    #[test]
    fn b1_bug_none_session_id_skips_id_match() {
        // B1 bug: agent_session_id was set to None before being passed to
        // find_best_session_file. This test verifies that when agent_session_id
        // is None, the function falls through to birthtime/fallback matching.
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("aaa.jsonl");
        let f2 = dir.path().join("bbb.jsonl");
        std::fs::File::create(&f1).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::File::create(&f2).unwrap();

        let agent = MockAgent {
            files: vec![f1.clone(), f2],
        };

        // With None session_id, should fall through to birthtime/oldest match.
        let result = find_best_session_file(&agent, "/tmp/test", &[], None, None);
        assert!(result.is_some());
    }
}
