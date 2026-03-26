use crate::config::ResolvedTheme;
use ansi_to_tui::IntoText;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render_terminal(
    frame: &mut Frame,
    area: Rect,
    content: &str,
    session_label: Option<&str>,
    terminal_focused: bool,
    theme: &ResolvedTheme,
) {
    let border_color = if terminal_focused {
        theme.border_focused
    } else {
        theme.border_unfocused
    };

    let title = match session_label {
        Some(label) => format!(" {label} "),
        None => " Terminal ".to_string(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    // Parse ANSI escape sequences into ratatui styled text.
    let text = content
        .as_bytes()
        .into_text()
        .unwrap_or_else(|_| Text::raw(content));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

pub fn render_empty_terminal(frame: &mut Frame, area: Rect, theme: &ResolvedTheme) {
    let block = Block::default()
        .title(" Terminal ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border_unfocused));

    let text = Text::styled(
        "No session selected",
        Style::default().fg(Color::DarkGray),
    );
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}
