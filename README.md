# chatmux

TUI session manager for AI coding agents. Run multiple [Claude Code](https://github.com/anthropics/claude-code) and [Codex](https://github.com/openai/codex) sessions side-by-side, with live status detection and macOS notifications.

## Features

- **Multi-session management** — Create, switch, rename, and delete agent sessions from a single terminal
- **Live status detection** — Monitors JSONL output files to detect Working / Replied / Read states
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
chatmux claude [args...]

# Launch a new Codex session directly
chatmux codex [args...]
```

## Keyboard Shortcuts

### Sidebar

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate up / down |
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

[theme]
border_focused = "cyan"
border_unfocused = "darkgray"
selected_fg = "cyan"
status_working = "blue"
status_replied = "red"
status_read = "green"
# Supports: color names, hex (#RRGGBB)
```

## License

MIT
