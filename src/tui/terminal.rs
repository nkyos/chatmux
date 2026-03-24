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
) {
    let border_color = if terminal_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let title = match session_label {
        Some(label) => format!(" {label} "),
        None => " Terminal ".to_string(),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    // For now, render the raw text from capture-pane.
    // ANSI codes will show as raw text until we add a parser.
    let text = Text::raw(content);
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

pub fn render_empty_terminal(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .title(" Terminal ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let text = Text::styled(
        "No session selected",
        Style::default().fg(Color::DarkGray),
    );
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}
