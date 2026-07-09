# chatmux

TUI session manager for AI coding agents. Run multiple [Claude Code](https://github.com/anthropics/claude-code) and [Codex](https://github.com/openai/codex) sessions side-by-side, with live status detection and macOS notifications.

## Features

- **Multi-session management** — Create, switch, rename, and delete agent sessions from a single terminal
- **Live status detection** — Uses Claude Code hooks for instant push-based status updates (Working / Replied / InputRequired / Read); JSONL polling as fallback for Codex
- **macOS notifications** — Get notified when an agent finishes, with a snippet of the reply
- **Session persistence** — Sessions survive app restarts (backed by tmux)
- **Project grouping** — View sessions grouped by project directory
- **Text selection & copy** — Select text with mouse, copy with `y` (via OSC 52)
- **Session history** — Browse and restart past sessions
- **Configurable** — Themes, notification sounds, sidebar width, editor integration

## Requirements

- macOS (notifications use `terminal-notifier` or `osascript`)
- [tmux](https://github.com/tmux/tmux)
- At least one supported agent:
  - `claude` ([Claude Code](https://github.com/anthropics/claude-code))
  - `codex` ([Codex CLI](https://github.com/openai/codex))

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Start the TUI
chatmux

# Launch a new Claude Code session directly
chatmux claude [--label "task description"] [args...]

# Launch a new Codex session directly
chatmux codex [--label "task description"] [args...]
```

## Keyboard Shortcuts

### Sidebar

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate up / down |
| `J` / `K` | Reorder session (manual sort mode) |
| `Enter` | Focus terminal |
| `n` | New session |
| `d` | Delete session |
| `r` | Rename / label session |
| `e` | Open in editor |
| `s` | Cycle sort mode |
| `/` | Filter sessions |
| `p` | Project view |
| `h` | History |
| `q` | Detach (keep sessions) |
| `Q` | Quit (kill sessions) |
| `?` | Help overlay |

### Terminal

| Key | Action |
|-----|--------|
| `Ctrl+]` then `Esc` | Back to sidebar |
| Mouse drag | Select text |
| `y` | Copy selection |
| `Esc` | Clear selection |
| Scroll | Scroll history |
| All other keys | Forwarded to agent |

## Configuration

`~/.config/chatmux/config.toml`

```toml
[editor]
command = "code"  # Defaults to $VISUAL > $EDITOR > cursor/code/zed > open

[notifications]
enabled = true
statuses = ["replied"]
sound = "default"

[display]
sidebar_width = 35

[agents]
skip_permissions = true          # Append --dangerously-skip-permissions / --yolo
claude_extra_args = []           # Extra args for Claude Code
codex_extra_args = []            # Extra args for Codex

[polling]
full_interval_secs = 30          # Full JSONL poll interval
watcher_debounce_ms = 300        # Filesystem watcher check interval
hook_check_ms = 300              # Hook event check interval
auto_save_secs = 30              # Auto-save state interval

[theme]
border_focused = "cyan"
border_unfocused = "darkgray"
selected_fg = "cyan"
status_working = "blue"
status_replied = "red"
status_read = "green"
# Supports: color names, hex (#RRGGBB)
```

## How Status Detection Works

**Claude Code sessions** use hooks (`--settings` with `SessionStart`, `UserPromptSubmit`, `Stop`, `Notification`) for instant push-based status updates. Each session gets a deterministic UUID (`--session-id`), so JSONL file association is exact — no heuristic matching needed.

**Codex sessions** fall back to JSONL file polling since Codex doesn't support hooks.

The sidebar shows a detection indicator next to each session: `⚡` for hooks-based, `~` for polling-based.

**Upstream dependencies:** chatmux reads Claude Code's JSONL files from `~/.claude/projects/<encoded-path>/` and Codex's from `~/.codex/sessions/YYYY/MM/DD/`. It also relies on Claude Code's hook event format (SessionStart, UserPromptSubmit, Stop, Notification). If an agent update changes these file layouts or event formats, status detection may break.

If hooks aren't working, check that:
- `~/.local/state/chatmux/hooks/claude-hook.sh` exists and is executable
- The `CHATMUX_SESSION` environment variable is set inside the tmux pane
- Press `x` in the sidebar to force a status re-read, or `X` to re-resolve the JSONL file
- Fallback JSONL polling will still work even if hooks are broken

## License

MIT
