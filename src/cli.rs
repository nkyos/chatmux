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

    // Generate a deterministic session ID for agents that support it.
    let session_uuid = uuid::Uuid::new_v4().to_string();
    let jsonl_path = agent.session_file_for(&cwd, &session_uuid);

    let mut args = agent.launch_args(Some(&session_uuid));
    args.extend(extra_args.iter().cloned());

    // Build command args list: env K=V ... command [args...]
    // Using separate arguments avoids shell interpretation (/bin/sh -c).
    let mut cmd_args: Vec<String> = vec!["env".to_string()];
    for (key, val) in TmuxClient::locale_env_pairs() {
        cmd_args.push(format!("{key}={val}"));
    }
    cmd_args.push(agent.command().to_string());
    cmd_args.extend(args);

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
        last_reply: None,
        session_file: jsonl_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        last_activity_epoch: Some(now_epoch()),
        status: Some("working".to_string()),
        jsonl_modified_epoch: None,
        jsonl_modified_nsec: None,
        jsonl_len: None,
        branch: detect_git_branch(&cwd),
        agent_session_id: Some(session_uuid),
        created_epoch: Some(now_epoch()),
    });
    state::save(&SavedState {
        sessions,
        next_id: next_id + 1,
    })?;

    // Create session + configure + attach in one tmux invocation.
    // Using non-detached mode so tmux uses the actual terminal size.
    // Passing command as separate arguments so tmux uses direct exec
    // instead of /bin/sh -c, avoiding shell injection.
    // Chained commands (\;) set session options after creation.
    let mut tmux = std::process::Command::new("tmux");
    tmux.args(["-u", "new-session", "-s", &full_name, "-c", &cwd]);
    for arg in &cmd_args {
        tmux.arg(arg);
    }
    tmux.args([";", "set", "mouse", "on", ";", "set", "history-limit", "50000"]);
    let err = tmux.exec();
    Err(anyhow::anyhow!("Failed to exec tmux: {err}"))
}
