use crate::config::ResolvedTheme;
use ansi_to_tui::IntoText;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Text,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Scroll state passed from the app to render the scrollbar.
pub struct TerminalScroll {
    /// Current scroll offset (0 = live view at bottom).
    pub offset: u16,
    /// Total scrollback history size.
    pub history_size: u16,
}

pub fn render_terminal(
    frame: &mut Frame,
    area: Rect,
    content: &str,
    session_label: Option<&str>,
    terminal_focused: bool,
    theme: &ResolvedTheme,
    scroll: Option<&TerminalScroll>,
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

    // Render scrollbar by painting directly into the buffer.
    if let Some(scroll) = scroll
        && scroll.history_size > 0 && scroll.offset > 0 {
            let track_height = area.height.saturating_sub(2) as usize;
            if track_height == 0 {
                return;
            }
            let total = scroll.history_size as usize + track_height;

            // Thumb size: proportional to viewport / total, minimum 1 row.
            let thumb_len = (track_height * track_height).div_ceil(total).max(1);

            // Scroll ratio: offset=0 is bottom, offset=history_size is top.
            // Map to thumb position where 0 = top of track.
            let scrollable = total.saturating_sub(track_height);
            let thumb_top = if scrollable > 0 {
                let max_thumb_top = track_height.saturating_sub(thumb_len);
                // offset goes from 0 (bottom) to history_size (top).
                // Invert: high offset = top of track = low thumb_top.
                max_thumb_top.saturating_sub(
                    (scroll.offset as usize * max_thumb_top + scrollable / 2) / scrollable,
                )
            } else {
                0
            };
            let thumb_bottom = thumb_top + thumb_len;

            // Paint on the right border column, inside top/bottom borders.
            let col = area.x + area.width - 1;
            let track_y = area.y + 1;
            let buf = frame.buffer_mut();
            for i in 0..track_height {
                let y = track_y + i as u16;
                if i >= thumb_top && i < thumb_bottom {
                    buf[(col, y)]
                        .set_symbol("┃")
                        .set_style(Style::default().fg(theme.border_focused));
                }
            }
        }
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
