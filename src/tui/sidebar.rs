use crate::config::ResolvedTheme;
use crate::session::model::{now_epoch, SessionStatus};
use crate::session::state::HistoryEntry;
use crate::session::{Session, SortMode};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Padding},
    Frame,
};

/// Aggregated project data for the project list view.
#[derive(Debug, Clone)]
pub struct ProjectSummary {
    pub cwd: String,
    pub project_name: String,
    pub session_count: usize,
    pub has_replied: bool,
    pub has_working: bool,
    pub aggregate_status: SessionStatus,
    pub latest_activity_epoch: u64,
}

pub fn render_sidebar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[Session],
    selected: Option<usize>,
    sidebar_focused: bool,
    theme: &ResolvedTheme,
    sort_mode: SortMode,
    filter: Option<&str>,
    rename: Option<(usize, &str)>,
    visible: &[usize],
    list_state: &mut ListState,
) {
    render_sidebar_with_title(
        frame,
        area,
        sessions,
        selected,
        sidebar_focused,
        theme,
        sort_mode,
        filter,
        rename,
        visible,
        list_state,
        None,
    );
}

pub fn render_sidebar_with_title(
    frame: &mut Frame,
    area: Rect,
    sessions: &[Session],
    selected: Option<usize>,
    sidebar_focused: bool,
    theme: &ResolvedTheme,
    sort_mode: SortMode,
    filter: Option<&str>,
    rename: Option<(usize, &str)>,
    visible: &[usize],
    list_state: &mut ListState,
    title_override: Option<&str>,
) {
    let border_color = if sidebar_focused {
        theme.border_focused
    } else {
        theme.border_unfocused
    };

    let title = title_override
        .map(|t| t.to_string())
        .unwrap_or_else(|| format!(" Sessions [{}] ", sort_mode.label()));

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1));

    if sessions.is_empty() && filter.is_none() {
        let items = vec![ListItem::new(Line::from(vec![
            Span::styled("  Press ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "n",
                Style::default()
                    .fg(theme.border_focused)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to create a session", Style::default().fg(Color::DarkGray)),
        ]))];
        let list = List::new(items).block(block);
        frame.render_stateful_widget(list, area, list_state);
        return;
    }

    let mut items: Vec<ListItem> = Vec::new();

    // Filter bar.
    if let Some(filter_text) = filter {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.border_focused)),
            Span::styled(filter_text.to_string(), Style::default().fg(Color::White)),
            Span::styled("▎", Style::default().fg(theme.border_focused)),
        ])));
        items.push(ListItem::new(Line::from("")));
    }

    for &idx in visible {
        let session = &sessions[idx];
        let is_selected = Some(idx) == selected;
        let is_renaming = rename.is_some_and(|(ri, _)| ri == idx);

        if is_renaming {
            let buf = rename.unwrap().1;
            items.push(session_to_rename_item(session, buf, theme));
        } else {
            items.push(session_to_list_item(session, is_selected, theme));
        }
    }

    if visible.is_empty() && filter.is_some() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  No matches",
            Style::default().fg(Color::DarkGray),
        ))));
    }

    // Calculate which list item index corresponds to the selected session.
    let selected_item_idx = selected.and_then(|sel| {
        visible.iter().position(|&i| i == sel).map(|pos| {
            let offset = if filter.is_some() { 2 } else { 0 };
            pos + offset
        })
    });
    list_state.select(selected_item_idx);

    let list = List::new(items).block(block);
    frame.render_stateful_widget(list, area, list_state);
}

/// Collapse newlines and trim whitespace from a label for single-line display.
fn sanitize_label(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replace home directory prefix with ~ and truncate from the start if too long.
fn truncate_path(path: &str) -> String {
    const MAX_LEN: usize = 36;
    let display = if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            format!("~{rest}")
        } else {
            path.to_string()
        }
    } else {
        path.to_string()
    };
    if display.len() <= MAX_LEN {
        return display;
    }
    // Keep the trailing part (more meaningful).
    let tail = &display[display.len() - MAX_LEN..];
    if let Some(slash_pos) = tail.find('/') {
        format!("…{}", &tail[slash_pos..])
    } else {
        format!("…{tail}")
    }
}

fn session_to_list_item(
    session: &Session,
    is_selected: bool,
    theme: &ResolvedTheme,
) -> ListItem<'static> {
    let select_indicator = if is_selected { "▶ " } else { "  " };
    let status_icon = session.status.icon();
    let agent_icon = session.agent_kind.icon();
    let elapsed = session.elapsed_display();

    let name_color = if is_selected {
        theme.selected_fg
    } else {
        Color::White
    };

    let agent_color = session.agent_kind.icon_color();

    // Line 1: agent icon + status icon + project name + elapsed
    let line1 = Line::from(vec![
        Span::raw(select_indicator.to_string()),
        Span::styled(agent_icon.to_string(), Style::default().fg(agent_color)),
        Span::raw(format!(" {status_icon} ")),
        Span::styled(
            session.project_name.clone(),
            Style::default().add_modifier(Modifier::BOLD).fg(name_color),
        ),
        Span::styled(format!("  {elapsed}"), Style::default().fg(Color::DarkGray)),
    ]);

    // Line 2: task label or last prompt (if available)
    let prompt_text = session
        .task_label
        .as_deref()
        .or(session.last_prompt.as_deref())
        .map(sanitize_label);

    // Line for cwd + branch
    let mut cwd_spans = vec![
        Span::raw("    "),
        Span::styled(truncate_path(&session.cwd), Style::default().fg(Color::DarkGray)),
    ];
    if let Some(ref branch) = session.branch {
        cwd_spans.push(Span::raw("  "));
        cwd_spans.push(Span::styled(
            format!(" {branch}"),
            Style::default().fg(Color::Cyan),
        ));
    }
    let cwd_line = Line::from(cwd_spans);

    let mut lines = vec![line1];
    if let Some(prompt) = prompt_text {
        lines.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(prompt, Style::default().fg(Color::Gray)),
        ]));
    }
    lines.push(cwd_line);
    lines.push(Line::from(""));

    ListItem::new(lines)
}

fn session_to_rename_item(
    session: &Session,
    buf: &str,
    theme: &ResolvedTheme,
) -> ListItem<'static> {
    let status_icon = session.status.icon();
    let agent_icon = session.agent_kind.icon();
    let agent_color = session.agent_kind.icon_color();

    let line1 = Line::from(vec![
        Span::raw("▶ ".to_string()),
        Span::styled(agent_icon.to_string(), Style::default().fg(agent_color)),
        Span::raw(format!(" {status_icon} ")),
        Span::styled(
            session.project_name.clone(),
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(theme.selected_fg),
        ),
    ]);

    let line2 = Line::from(vec![
        Span::raw("    "),
        Span::styled(
            format!("{buf}▎"),
            Style::default().fg(Color::White).bg(Color::DarkGray),
        ),
    ]);

    let mut cwd_spans = vec![
        Span::raw("    "),
        Span::styled(truncate_path(&session.cwd), Style::default().fg(Color::DarkGray)),
    ];
    if let Some(ref branch) = session.branch {
        cwd_spans.push(Span::raw("  "));
        cwd_spans.push(Span::styled(
            format!(" {branch}"),
            Style::default().fg(Color::Cyan),
        ));
    }
    let line3 = Line::from(cwd_spans);

    let line4 = Line::from("");

    ListItem::new(vec![line1, line2, line3, line4])
}

/// Render the summary bar at the bottom of the sidebar.
pub fn render_summary_bar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[Session],
    theme: &ResolvedTheme,
) {
    let mut replied = 0u32;
    let mut read = 0u32;
    let mut working = 0u32;

    for s in sessions {
        match s.status {
            SessionStatus::Replied => replied += 1,
            SessionStatus::Read => read += 1,
            SessionStatus::Working => working += 1,
        }
    }

    let spans = vec![
        Span::styled(
            format!(" 🔴{replied}"),
            Style::default().fg(theme.status_replied),
        ),
        Span::raw("  "),
        Span::styled(
            format!("⏳{working}"),
            Style::default().fg(theme.status_working),
        ),
        Span::raw("  "),
        Span::styled(
            format!("✅{read}"),
            Style::default().fg(theme.status_read),
        ),
    ];

    let line = Line::from(spans);
    frame.render_widget(line, area);
}

fn elapsed_display_from_epoch(epoch: u64) -> String {
    let secs = now_epoch().saturating_sub(epoch);
    if secs < 60 {
        "now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else {
        format!("{}h ago", secs / 3600)
    }
}

/// Render the project list view in the sidebar.
pub fn render_project_list(
    frame: &mut Frame,
    area: Rect,
    projects: &[ProjectSummary],
    selected: usize,
    sidebar_focused: bool,
    theme: &ResolvedTheme,
    list_state: &mut ListState,
) {
    let border_color = if sidebar_focused {
        theme.border_focused
    } else {
        theme.border_unfocused
    };

    let block = Block::default()
        .title(format!(" Projects [{}] ", projects.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1));

    if projects.is_empty() {
        let items = vec![ListItem::new(Line::from(Span::styled(
            "  No active projects",
            Style::default().fg(Color::DarkGray),
        )))];
        let list = List::new(items).block(block);
        frame.render_stateful_widget(list, area, list_state);
        return;
    }

    let items: Vec<ListItem> = projects
        .iter()
        .enumerate()
        .map(|(i, proj)| {
            let is_selected = i == selected;
            let select_indicator = if is_selected { "▶ " } else { "  " };

            let name_color = if is_selected {
                theme.selected_fg
            } else {
                Color::White
            };

            let status_icon = proj.aggregate_status.icon();
            let elapsed = elapsed_display_from_epoch(proj.latest_activity_epoch);

            // Line 1: status icon + project name + elapsed
            let line1 = Line::from(vec![
                Span::raw(select_indicator.to_string()),
                Span::raw(format!("{status_icon} ")),
                Span::styled(
                    proj.project_name.clone(),
                    Style::default().add_modifier(Modifier::BOLD).fg(name_color),
                ),
                Span::styled(format!("  {elapsed}"), Style::default().fg(Color::DarkGray)),
            ]);

            // Line 2: session count + status badges
            let mut badges = vec![Span::raw("    ")];
            badges.push(Span::styled(
                format!("{} sessions", proj.session_count),
                Style::default().fg(Color::DarkGray),
            ));
            if proj.has_replied {
                badges.push(Span::raw("  "));
                badges.push(Span::styled(
                    "🔴 unread",
                    Style::default().fg(theme.status_replied),
                ));
            }
            if proj.has_working {
                badges.push(Span::raw("  "));
                badges.push(Span::styled(
                    "⏳ working",
                    Style::default().fg(theme.status_working),
                ));
            }
            let line2 = Line::from(badges);

            // Line 3: cwd
            let line3 = Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    truncate_path(&proj.cwd),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            let line4 = Line::from("");

            ListItem::new(vec![line1, line2, line3, line4])
        })
        .collect();

    list_state.select(Some(selected));

    let list = List::new(items).block(block);
    frame.render_stateful_widget(list, area, list_state);
}

pub fn render_history_sidebar(
    frame: &mut Frame,
    area: Rect,
    entries: &[HistoryEntry],
    selected: usize,
    sidebar_focused: bool,
    theme: &ResolvedTheme,
    list_state: &mut ListState,
) {
    let border_color = if sidebar_focused {
        theme.border_focused
    } else {
        theme.border_unfocused
    };

    let block = Block::default()
        .title(" History ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .padding(Padding::horizontal(1));

    if entries.is_empty() {
        let items = vec![ListItem::new(Line::from(Span::styled(
            "  No history",
            Style::default().fg(Color::DarkGray),
        )))];
        let list = List::new(items).block(block);
        frame.render_stateful_widget(list, area, list_state);
        return;
    }

    let items: Vec<ListItem> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let is_selected = i == selected;
            let select_indicator = if is_selected { "▶ " } else { "  " };

            let name_color = if is_selected {
                theme.selected_fg
            } else {
                Color::Gray
            };

            let elapsed = entry.elapsed_display();
            let agent_icon = entry.agent_kind.icon();
            let agent_color = entry.agent_kind.icon_color();

            let line1 = Line::from(vec![
                Span::raw(select_indicator.to_string()),
                Span::styled(agent_icon.to_string(), Style::default().fg(agent_color)),
                Span::raw(" "),
                Span::styled(
                    entry.project_name.clone(),
                    Style::default().fg(name_color),
                ),
                Span::styled(format!("  {elapsed}"), Style::default().fg(Color::DarkGray)),
            ]);

            let label = entry
                .task_label
                .as_deref()
                .or(entry.last_prompt.as_deref())
                .unwrap_or(&entry.cwd);
            let line2 = Line::from(vec![
                Span::raw("    "),
                Span::styled(label.to_string(), Style::default().fg(Color::DarkGray)),
            ]);

            ListItem::new(vec![line1, line2])
        })
        .collect();

    list_state.select(Some(selected));

    let list = List::new(items).block(block);
    frame.render_stateful_widget(list, area, list_state);
}
