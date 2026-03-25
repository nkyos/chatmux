mod app;
mod config;
mod jsonl;
mod notify;
mod projects;
mod session;
mod tmux;
mod tui;

use anyhow::Result;
use app::App;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_loop(&mut terminal, &mut app);

    app.cleanup();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
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
