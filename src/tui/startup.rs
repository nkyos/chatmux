use ratatui::{
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

pub fn render_startup_screen(
    frame: &mut Frame,
    area: Rect,
    session_names: &[String],
    cold_restore: bool,
) {
    let count = session_names.len();

    // Build content lines.
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            if cold_restore {
                format!("  {count} saved session(s) found")
            } else {
                format!("  {count} existing session(s) found")
            },
            Style::default().fg(Color::Yellow),
        )),
    ];

    if cold_restore {
        lines.push(Line::from(Span::styled(
            "  (will resume agents in new terminals)",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));

    // Show session names (up to 8).
    for (i, name) in session_names.iter().take(8).enumerate() {
        let prefix = if i < session_names.len().min(8) {
            "    "
        } else {
            ""
        };
        lines.push(Line::from(Span::styled(
            format!("{prefix}• {name}"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    if count > 8 {
        lines.push(Line::from(Span::styled(
            format!("    … and {} more", count - 8),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  r", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        if cold_restore {
            Span::raw("  Restore & resume agents")
        } else {
            Span::raw("  Restore previous sessions")
        },
    ]));
    lines.push(Line::from(vec![
        Span::styled("  n", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::raw("  Start fresh (discard all)"),
    ]));
    lines.push(Line::from(""));

    let height = lines.len() as u16 + 2; // +2 for borders
    let width = 42u16;

    // Center the popup.
    let popup = centered_rect(width, height, area);

    let block = Block::default()
        .title(" chatmux ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(lines).block(block);

    frame.render_widget(Clear, popup);
    frame.render_widget(paragraph, popup);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(area);
    let horizontal = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vertical[0]);
    horizontal[0]
}
