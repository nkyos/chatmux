use crate::agent::{AgentKind, AgentRegistry};
use crate::session::model::{detect_git_branch, now_epoch};
use crate::session::state::{self, SavedState, SessionEntry};
use crate::tmux::TmuxClient;
use anyhow::Result;
use std::os::unix::process::CommandExt;
use std::path::Path;

/// Run an agent in a chatmux-managed tmux session and attach to it.
///
/// This creates a tmux session with the chatmux naming convention so
/// the chatmux TUI can discover and manage it.
pub fn run_attach(kind: AgentKind, extra_args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir()?
        .to_string_lossy()
        .into_owned();
    let registry = AgentRegistry::new();
    let agent = registry.get(kind);

    // Load existing state to determine next session ID.
    let (next_id, mut sessions) = match state::load() {
        Some(saved) => (saved.next_id, saved.sessions),
        None => (0, vec![]),
    };

    let name = format!("s{next_id}");
    let full_name = format!("chatmux-{name}");

    // Build args: agent defaults + extra user args.
    let mut args = agent.args();
    args.extend(extra_args.iter().cloned());

    // Build shell command with locale env prefix.
    let env_prefix = TmuxClient::locale_env_prefix();
    let shell_cmd = if args.is_empty() {
        format!("{env_prefix}{}", agent.command())
    } else {
        format!("{env_prefix}{} {}", agent.command(), args.join(" "))
    };

    // Derive project name from cwd.
    let project_name = Path::new(&cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.clone());

    // Save state BEFORE creating the session so TUI can discover it.
    sessions.push(SessionEntry {
        name: name.clone(),
        cwd: cwd.clone(),
        project_name,
        agent_kind: kind,
        task_label: None,
        last_prompt: None,
        session_file: None,
        last_activity_epoch: Some(now_epoch()),
        status: Some("working".to_string()),
        jsonl_modified_epoch: None,
        jsonl_modified_nsec: None,
        branch: detect_git_branch(&cwd),
    });
    state::save(&SavedState {
        sessions,
        next_id: next_id + 1,
    })?;

    // Create session + configure + attach in one tmux invocation.
    // Using non-detached mode so tmux uses the actual terminal size.
    // Chained commands (\;) set session options after creation.
    let err = std::process::Command::new("tmux")
        .args([
            "-u",
            "new-session",
            "-s", &full_name,
            "-c", &cwd,
            &shell_cmd,
            ";",  // tmux command separator
            "set", "mouse", "on",
            ";",
            "set", "history-limit", "50000",
        ])
        .exec();
    Err(anyhow::anyhow!("Failed to exec tmux: {err}"))
}
