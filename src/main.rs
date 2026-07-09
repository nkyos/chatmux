mod agent;
mod app;
mod cli;
mod config;
mod hooks;
mod notify;
mod projects;
mod session;
mod spool;
mod tmux;
mod tui;

use anyhow::Result;
use app::App;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() > 1 {
        match args[1].as_str() {
            "claude" => return cli::run_attach(agent::AgentKind::ClaudeCode, &args[2..]),
            "codex" => return cli::run_attach(agent::AgentKind::Codex, &args[2..]),
            other => {
                eprintln!("Unknown subcommand: {other}");
                eprintln!("Usage: chatmux [claude|codex] [args...]");
                std::process::exit(1);
            }
        }
    }

    run_tui()
}

fn run_tui() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;

    // Enable Kitty keyboard protocol for proper key disambiguation.
    // supports_keyboard_enhancement() can give false negatives, so we
    // unconditionally push and silently ignore failure on cleanup.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    // Use catch_unwind so cleanup (including state save) runs even on panic.
    let result = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_loop(&mut terminal, &mut app)
    })) {
        Ok(r) => r,
        Err(payload) => {
            // Save state before re-raising the panic so sessions can be restored.
            // Don't call cleanup() — we want to keep tmux sessions alive.
            app.save_state_for_crash_recovery();
            std::panic::resume_unwind(payload);
        }
    };

    app.cleanup();
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    loop {
        // Update layout first so tmux panes are resized before capture.
        let size = terminal.size()?;
        let size_rect = Rect::new(0, 0, size.width, size.height);
        app.update_layout(size_rect);

        app.tick();

        terminal.draw(|frame| {
            app.draw(frame);
        })?;

        app.handle_event()?;

        if app.should_quit() {
            return Ok(());
        }
    }
}
