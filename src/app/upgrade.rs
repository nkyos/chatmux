use super::*;

impl App {
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
        self.terminal.content.clear();
        crate::session::state::remove();
    }

    /// Recreate sessions from a snapshot using resume commands.
    pub(super) fn recreate_from_snapshot(&mut self, snapshot: &[RestartEntry]) -> Result<()> {
        let width = self.terminal.area.width.saturating_sub(2);
        let height = self.terminal.area.height.saturating_sub(2);

        for entry in snapshot {
            let agent = self.registry.get(entry.agent_kind);
            let opts = self.agent_opts(entry.agent_kind);
            let idx = self.manager.create_resume(
                &entry.cwd,
                agent,
                entry.agent_session_id.as_deref(),
                width,
                height,
                &opts,
            )?;
            if let Some(ref label) = entry.task_label
                && let Some(session) = self.manager.sessions_mut().get_mut(idx) {
                    session.task_label = Some(label.clone());
                }
        }

        if !self.manager.is_empty() {
            self.select_by_index(0);
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
        let width = self.terminal.area.width.saturating_sub(2);
        let height = self.terminal.area.height.saturating_sub(2);

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

        if kinds.is_empty() {
            for agent in self.registry.available() {
                kinds.insert(agent.kind());
            }
        }

        let mut commands = Vec::new();
        if kinds.contains(&AgentKind::ClaudeCode) {
            commands.push(resolve_upgrade_command(
                self.config.upgrade.claude_code.as_deref(),
                "claude-code",
                "claude",
                "@anthropic-ai/claude-code",
                "claude-code",
            ));
        }
        if kinds.contains(&AgentKind::Codex) {
            commands.push(resolve_upgrade_command(
                self.config.upgrade.codex.as_deref(),
                "codex",
                "codex",
                "@openai/codex",
                "codex",
            ));
        }

        if commands.is_empty() {
            "echo 'No agents to upgrade'".into()
        } else {
            commands.join("\n")
        }
    }
}

/// Resolve an upgrade command for a single agent. If the user supplied an
/// override in config it is used verbatim; otherwise a shell snippet is
/// generated that detects the install method at runtime and prefers, in order:
/// mise → Homebrew → npm global.
fn resolve_upgrade_command(
    override_cmd: Option<&str>,
    label: &str,
    bin: &str,
    npm_pkg: &str,
    brew_formula: &str,
) -> String {
    if let Some(cmd) = override_cmd {
        return cmd.to_string();
    }
    format!(
        r#"echo "== Upgrading {label} =="
if ! command -v {bin} >/dev/null 2>&1; then
  echo "{bin}: not installed, skipping"
elif command -v mise >/dev/null 2>&1 && mise which {bin} >/dev/null 2>&1; then
  echo "-> detected mise"
  mise upgrade 'npm:{npm_pkg}'
elif command -v brew >/dev/null 2>&1 && brew list {brew_formula} >/dev/null 2>&1; then
  echo "-> detected homebrew"
  brew upgrade {brew_formula}
elif command -v npm >/dev/null 2>&1; then
  echo "-> fallback to npm global"
  npm install -g '{npm_pkg}@latest'
else
  echo "{label}: no supported package manager found (mise/brew/npm)"
fi"#
    )
}
