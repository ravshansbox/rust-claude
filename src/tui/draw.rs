use super::{
    App, display_model,
    input::{input_cursor, input_rows},
    selection::highlight_selection,
    status::{format_context, format_quota, format_stats},
};
use conversation::{conversation_layout, held_scroll_from_bottom};
use lists::{draw_panel, draw_suggestions, panel};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::borrow::Cow;

mod conversation;
mod lists;

pub(super) use conversation::conversation;

const BOTTOM_BUTTON: &str = "↓ Bottom (End)";
pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let input_width = usize::from(frame.area().width.max(1));
    app.input_width = input_width;
    let input_rows = input_rows(&app.input, input_width);
    let (cursor_row, cursor_column) = input_cursor(&app.input, app.cursor, input_width);
    let input_lines: Vec<Line> = input_rows.into_iter().map(Line::raw).collect();
    let footer_paragraph = footer(app);
    let footer_height = saturating_u16(footer_paragraph.line_count(frame.area().width));
    let [chat, input, footer] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(saturating_u16(input_lines.len()).saturating_add(2)),
        Constraint::Length(footer_height),
    ])
    .areas(frame.area());

    for message in &mut app.messages {
        message.render(chat.width);
    }

    let panel = panel(app.picker.as_ref(), app.history_search.as_ref(), chat);
    let panel_height = panel.as_ref().map_or(0, |(_, height, _)| *height);
    let mut pinned = app.scroll_from_bottom > 0;
    let (mut pinned_lines, mut pinned_height, mut conversation_area, mut max_scroll) =
        conversation_layout(app, chat, panel_height, pinned);
    app.scroll_from_bottom = if pinned && !app.status_pinned {
        app.scroll_from_bottom.min(max_scroll)
    } else {
        held_scroll_from_bottom(app.scroll_from_bottom, app.max_scroll, max_scroll)
    };
    if pinned && app.scroll_from_bottom == 0 {
        pinned = false;
        (pinned_lines, pinned_height, conversation_area, max_scroll) =
            conversation_layout(app, chat, panel_height, false);
    }
    let viewport_height = conversation_area.height;
    app.status_pinned = pinned;
    app.max_scroll = max_scroll;
    app.page_size = usize::from(viewport_height.max(1));
    let scroll = app.max_scroll.saturating_sub(app.scroll_from_bottom);
    app.conversation_area = conversation_area;
    app.conversation_top = scroll;
    frame.render_widget(
        conversation(app, chat.width, scroll, viewport_height),
        conversation_area,
    );
    highlight_selection(app, frame.buffer_mut());
    let pinned_area = Rect {
        y: conversation_area.y + conversation_area.height,
        height: pinned_height,
        ..chat
    };
    frame.render_widget(
        Paragraph::new(pinned_lines).wrap(Wrap { trim: false }),
        pinned_area,
    );
    app.bottom_button = draw_bottom_button(frame, chat, pinned_area);

    draw_panel(frame, chat, panel);

    draw_suggestions(frame, app, chat);

    draw_input(frame, input, input_lines, cursor_row, cursor_column);

    frame.render_widget(footer_paragraph, footer);
}

fn draw_bottom_button(frame: &mut Frame, chat: Rect, pinned_area: Rect) -> Rect {
    if pinned_area.height == 0 {
        return Rect::default();
    }
    let width = saturating_u16(Line::raw(BOTTOM_BUTTON).width()).min(chat.width);
    let button = Rect {
        x: chat.x + chat.width - width,
        width,
        height: 1,
        ..pinned_area
    };
    frame.render_widget(Clear, button);
    frame.render_widget(Paragraph::new(BOTTOM_BUTTON).dark_gray(), button);
    button
}

fn draw_input(
    frame: &mut Frame,
    input: Rect,
    input_lines: Vec<Line>,
    cursor_row: usize,
    cursor_column: usize,
) {
    let cursor_row = saturating_u16(cursor_row);
    let cursor_column = saturating_u16(cursor_column);
    let input_scroll = cursor_row.saturating_sub(input.height.saturating_sub(3));
    frame.render_widget(
        Paragraph::new(input_lines)
            .block(Block::default().borders(Borders::TOP | Borders::BOTTOM))
            .scroll((input_scroll, 0)),
        input,
    );
    frame.set_cursor_position((
        input.x + cursor_column,
        input.y + cursor_row - input_scroll + 1,
    ));
}

fn footer(app: &App) -> Paragraph<'static> {
    let mut footer_parts: Vec<Cow<str>> = vec![
        Cow::Borrowed(&app.workspace),
        format!("{}:{}", display_model(&app.model), app.effort).into(),
        match app.tasks.len() {
            0 => Cow::Borrowed(""),
            1 => Cow::Borrowed("1 agent"),
            count => format!("{count} agents").into(),
        },
        match app.stats.background {
            0 => Cow::Borrowed(""),
            count => format!("{count} background").into(),
        },
        format_context(&app.stats).into(),
        format_stats(&app.stats).into(),
        format_quota(&app.stats).into(),
    ];
    footer_parts.retain(|part| !part.is_empty());
    Paragraph::new(Line::raw(footer_parts.join(" · "))).wrap(Wrap { trim: false })
}

#[cfg(test)]
mod tests;
