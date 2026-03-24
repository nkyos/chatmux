mod app;
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
use ratatui::{Terminal, backend::CrosstermBackend};

fn main() -> Result<()> {
    // Setup terminal.
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = run_loop(&mut terminal, &mut app);

    // Cleanup: restore terminal and kill tmux sessions.
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
        app.tick();

        terminal.draw(|frame| {
            app.draw(frame);
        })?;

        // Resize the selected tmux pane to match the terminal view area.
        let size = terminal.size()?;
        let size_rect = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        let terminal_area = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Length(35),
                ratatui::layout::Constraint::Min(1),
            ])
            .split(size_rect)[1];
        app.resize_selected_pane(terminal_area);

        app.handle_event()?;

        if app.should_quit() {
            return Ok(());
        }
    }
}
