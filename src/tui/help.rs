use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph},
    Frame,
};

/// Which context the help overlay should show shortcuts for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelpContext {
    Sessions,
    Projects,
    ProjectSessions,
    Terminal,
    History,
}

fn shortcut_lines(ctx: HelpContext) -> Vec<(&'static str, &'static str)> {
    let mut lines = Vec::new();

    match ctx {
        HelpContext::Sessions => {
            lines.push(("j / k", "Move up / down"));
            lines.push(("J / K", "Reorder (manual sort)"));
            lines.push(("Enter", "Focus terminal"));
            lines.push(("n", "New session"));
            lines.push(("d", "Delete session"));
            lines.push(("r", "Rename session"));
            lines.push(("e", "Open in editor"));
            lines.push(("s", "Cycle sort mode"));
            lines.push(("/", "Filter sessions"));
            lines.push(("x", "Refresh status"));
            lines.push(("X", "Re-resolve JSONL"));
            lines.push(("p", "Project view"));
            lines.push(("h", "History"));
            lines.push(("U", "Upgrade + restart all"));
            lines.push(("R", "Restart all sessions"));
            lines.push(("q", "Detach (keep sessions)"));
            lines.push(("Q", "Quit (kill sessions)"));
            lines.push(("", ""));
            lines.push(("⚡", "Hooks detection"));
            lines.push(("~", "Polling detection"));
        }
        HelpContext::Projects => {
            lines.push(("j / k", "Move up / down"));
            lines.push(("Enter", "Open project sessions"));
            lines.push(("n", "New session"));
            lines.push(("U", "Upgrade + restart all"));
            lines.push(("R", "Restart all sessions"));
            lines.push(("p / Esc", "Back to sessions"));
            lines.push(("q", "Detach (keep sessions)"));
            lines.push(("Q", "Quit (kill sessions)"));
        }
        HelpContext::ProjectSessions => {
            lines.push(("j / k", "Move up / down"));
            lines.push(("Enter", "Focus terminal"));
            lines.push(("d", "Delete session"));
            lines.push(("r", "Rename session"));
            lines.push(("e", "Open in editor"));
            lines.push(("n", "New session"));
            lines.push(("U", "Upgrade + restart all"));
            lines.push(("R", "Restart all sessions"));
            lines.push(("Esc", "Back to projects"));
            lines.push(("p", "Back to sessions"));
            lines.push(("q", "Detach (keep sessions)"));
            lines.push(("Q", "Quit (kill sessions)"));
        }
        HelpContext::Terminal => {
            lines.push(("C-] Esc", "Back to sidebar"));
            lines.push(("Scroll", "Scroll history"));
            lines.push(("y", "Copy selection"));
            lines.push(("Esc", "Clear selection"));
            lines.push(("*", "All keys forwarded to agent"));
        }
        HelpContext::History => {
            lines.push(("j / k", "Move up / down"));
            lines.push(("Enter", "Restart session"));
            lines.push(("d", "Delete entry"));
            lines.push(("h / Esc", "Back to sessions"));
            lines.push(("q", "Detach (keep sessions)"));
            lines.push(("Q", "Quit (kill sessions)"));
        }
    }

    lines.push(("?", "Toggle this help"));
    lines
}

/// Render a centered help overlay popup.
pub fn render_help_overlay(frame: &mut Frame, area: Rect, ctx: HelpContext) {
    let shortcuts = shortcut_lines(ctx);

    // Size the popup.
    let width = 40u16.min(area.width.saturating_sub(4));
    let height = (shortcuts.len() as u16 + 4).min(area.height.saturating_sub(2));

    let popup_area = centered_rect(width, height, area);

    // Clear the area behind the popup.
    frame.render_widget(Clear, popup_area);

    let title = match ctx {
        HelpContext::Sessions => " Shortcuts — Sessions ",
        HelpContext::Projects => " Shortcuts — Projects ",
        HelpContext::ProjectSessions => " Shortcuts — Project Sessions ",
        HelpContext::Terminal => " Shortcuts — Terminal ",
        HelpContext::History => " Shortcuts — History ",
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .padding(Padding::horizontal(1));

    let lines: Vec<Line> = shortcuts
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("{:<12}", key),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*desc, Style::default().fg(Color::White)),
            ])
        })
        .collect();

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}

/// Render a confirmation overlay popup (e.g. "Upgrade and restart N sessions? [y/n]").
pub fn render_confirm_overlay(frame: &mut Frame, area: Rect, message: &str) {
    let width = 50u16.min(area.width.saturating_sub(4));
    let height = 5u16.min(area.height.saturating_sub(2));
    let popup_area = centered_rect(width, height, area);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Confirm ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .padding(Padding::horizontal(1));

    let lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(message, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(
                "  [y]es  [n]o",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    ];

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup_area);
}

/// Return a centered `Rect` of the given size within `area`.
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .split(area);
    Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .split(vertical[0])[0]
}
