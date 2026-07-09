use crate::agent::{AgentKind, AgentRegistry};
use crate::session::model::{detect_git_branch, now_epoch};
use crate::spool::{SpoolEntry, write_spool};
use crate::tmux::TmuxClient;
use anyhow::Result;
use std::os::unix::process::CommandExt;
use std::path::Path;

/// Run an agent in a chatmux-managed tmux session and attach to it.
///
/// This creates a tmux session with the chatmux naming convention so
/// the chatmux TUI can discover and manage it. Metadata is written to
/// a spool file (`pending/{name}.json`) instead of sessions.json —
/// the TUI picks it up on discovery.
pub fn run_attach(kind: AgentKind, extra_args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir()?
        .to_string_lossy()
        .into_owned();
    let registry = AgentRegistry::new();
    let agent = registry.get(kind);

    // Parse --label <text> before passing remaining args to the agent.
    let (task_label, agent_args) = parse_label_arg(extra_args);

    // Use a UUID-based name to avoid next_id contention with the TUI.
    let short_uuid = &uuid::Uuid::new_v4().to_string()[..8];
    let name = format!("x{short_uuid}");
    let full_name = format!("chatmux-{name}");

    // Generate a deterministic session ID for agents that support it.
    let session_uuid = uuid::Uuid::new_v4().to_string();
    let jsonl_path = agent.session_file_for(&cwd, &session_uuid);

    let mut args = agent.launch_args(Some(&session_uuid));
    args.extend(agent_args.iter().cloned());

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

    // Write spool file so TUI can pick up metadata on discovery.
    let spool = SpoolEntry {
        cwd: cwd.clone(),
        project_name,
        agent_kind: kind,
        agent_session_id: Some(session_uuid),
        session_file: jsonl_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
        task_label: task_label.clone(),
        created_epoch: now_epoch(),
        branch: detect_git_branch(&cwd),
    };
    let _ = write_spool(&name, &spool);

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

/// Extract `--label <text>` from the argument list, returning (label, remaining_args).
fn parse_label_arg(args: &[String]) -> (Option<String>, Vec<String>) {
    let mut label = None;
    let mut remaining = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--label" {
            label = iter.next().cloned();
        } else {
            remaining.push(arg.clone());
        }
    }
    (label, remaining)
}
