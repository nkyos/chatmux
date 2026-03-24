use crate::session::{Session, SessionStatus};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Padding},
    Frame,
};

pub fn render_sidebar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[Session],
    selected: Option<usize>,
    sidebar_focused: bool,
) {
    let border_color = if sidebar_focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .title(" Sessions ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1));

    if sessions.is_empty() {
        let items = vec![ListItem::new(Line::from(vec![
            Span::styled("  Press ", Style::default().fg(Color::DarkGray)),
            Span::styled("n", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::styled(" to create a session", Style::default().fg(Color::DarkGray)),
        ]))];
        let list = List::new(items).block(block);
        frame.render_widget(list, area);
        return;
    }

    let items: Vec<ListItem> = sessions
        .iter()
        .enumerate()
        .map(|(i, session)| session_to_list_item(session, Some(i) == selected))
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

fn session_to_list_item(session: &Session, is_selected: bool) -> ListItem<'static> {
    let select_indicator = if is_selected { "▶ " } else { "  " };
    let icon = session.status.icon();
    let elapsed = session.elapsed_display();

    // Line 1: icon + project name + elapsed
    let line1 = Line::from(vec![
        Span::raw(select_indicator.to_string()),
        Span::raw(format!("{icon} ")),
        Span::styled(
            session.project_name.clone(),
            Style::default().add_modifier(Modifier::BOLD).fg(if is_selected {
                Color::Cyan
            } else {
                Color::White
            }),
        ),
        Span::styled(format!("  {elapsed}"), Style::default().fg(Color::DarkGray)),
    ]);

    // Line 2: task label
    let label = session.display_label().to_string();
    let line2 = Line::from(vec![
        Span::raw("    "),
        Span::styled(label, Style::default().fg(Color::Gray)),
    ]);

    // Line 3: empty line as separator
    let line3 = Line::from("");

    ListItem::new(vec![line1, line2, line3])
}

/// Render the summary bar at the bottom of the sidebar.
pub fn render_summary_bar(frame: &mut Frame, area: Rect, sessions: &[Session]) {
    let mut waiting = 0u32;
    let mut replied = 0u32;
    let mut working = 0u32;
    let mut idle = 0u32;

    for s in sessions {
        match s.status {
            SessionStatus::Waiting => waiting += 1,
            SessionStatus::Replied => replied += 1,
            SessionStatus::Working => working += 1,
            SessionStatus::Idle => idle += 1,
        }
    }

    let spans = vec![
        Span::styled(format!(" ⚠️{waiting}"), Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(format!("🔴{replied}"), Style::default().fg(Color::Red)),
        Span::raw("  "),
        Span::styled(format!("⏳{working}"), Style::default().fg(Color::Blue)),
        Span::raw("  "),
        Span::styled(format!("💤{idle}"), Style::default().fg(Color::DarkGray)),
    ];

    let line = Line::from(spans);
    frame.render_widget(line, area);
}
