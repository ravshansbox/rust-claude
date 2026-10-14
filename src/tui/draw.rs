use super::{
    App, display_model,
    input::{input_cursor, input_rows},
    render::wrapped_height,
    selection::highlight_selection,
    status::{format_context, format_quota, format_stats},
};
use lists::{draw_panel, draw_suggestions, panel};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    text::{Line, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::borrow::Cow;

mod lists;

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

fn conversation_layout(
    app: &App,
    chat: Rect,
    panel_height: u16,
    pinned: bool,
) -> (Vec<Line<'static>>, u16, Rect, usize) {
    let pinned_lines: Vec<Line> = if pinned {
        let lines: Vec<Line> = status_lines(app).into_iter().skip(1).collect();
        if lines.is_empty() {
            vec![Line::default()]
        } else {
            lines
        }
    } else {
        Vec::new()
    };
    let pinned_height = saturating_u16(
        pinned_lines
            .iter()
            .map(|line| wrapped_height(line, chat.width))
            .sum::<usize>(),
    )
    .min((chat.height - panel_height).saturating_sub(1));
    let conversation_area = Rect {
        height: chat.height - panel_height - pinned_height,
        ..chat
    };
    let wrapped_line_count: usize = app
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| app.gap_before(index) + message.height())
        .sum::<usize>()
        + if pinned {
            0
        } else {
            status_lines(app)
                .iter()
                .map(|line| wrapped_height(line, chat.width))
                .sum::<usize>()
        };
    let max_scroll = wrapped_line_count.saturating_sub(usize::from(conversation_area.height));
    (pinned_lines, pinned_height, conversation_area, max_scroll)
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

fn status_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(activity) = app.activity {
        lines.push(Line::default());
        let frame = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
        lines.push(Line::from(format!("{frame} {activity}").dark_gray()));
        if let Ok(queue) = app.queue.lock() {
            for queued in queue.iter() {
                let first_line = queued.prompt.lines().next().unwrap_or_default();
                lines.push(Line::from(format!("queued: {first_line}").dark_gray()));
            }
        }
    }
    lines
}

/// The conversation `width` columns wide, from row `top` for `height_shown` rows.
pub(super) fn conversation(app: &App, width: u16, top: usize, height_shown: u16) -> Paragraph<'_> {
    let status_lines = if app.status_pinned {
        Vec::new()
    } else {
        status_lines(app)
    };
    let status_heights: Vec<usize> = status_lines
        .iter()
        .map(|line| wrapped_height(line, width))
        .collect();
    // Paragraph offsets are u16, so drop the lines above the view and scroll
    // only within the first visible one. Lines below the view are left out.
    let mut skipped = 0;
    let mut first_message = 0;
    while let Some(message) = app.messages.get(first_message)
        && skipped + app.gap_before(first_message) + message.height() <= top
    {
        skipped += app.gap_before(first_message) + message.height();
        first_message += 1;
    }
    // Wrapping lines is slow, so skip the whole lines above the view in the
    // first visible message.
    let first_rows = if let Some(rendered) = app
        .messages
        .get(first_message)
        .and_then(|message| message.rendered.as_ref())
        && top > skipped
    {
        let gap = app.gap_before(first_message);
        let (rows_skipped, rows) = rendered.rows_from(top - skipped - gap);
        skipped += gap + rows_skipped;
        Some(rows)
    } else {
        None
    };
    let rest = first_message + usize::from(first_rows.is_some());
    let rows = first_rows
        .into_iter()
        .flatten()
        .chain(
            app.messages[rest..]
                .iter()
                .enumerate()
                .flat_map(move |(offset, message)| {
                    let rendered = message
                        .rendered
                        .iter()
                        .flat_map(|rendered| rendered.rows_from(0).1);
                    std::iter::repeat_n((Line::default(), 1), app.gap_before(rest + offset))
                        .chain(rendered)
                }),
        )
        .chain(status_lines.into_iter().zip(status_heights));
    let mut lines = Vec::new();
    let mut filled = 0;
    for (line, height) in rows {
        if lines.is_empty() && skipped + height <= top {
            skipped += height;
            continue;
        }
        if filled >= top - skipped + usize::from(height_shown) {
            break;
        }
        filled += height;
        lines.push(line);
    }
    let offset = saturating_u16(top - skipped);
    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((offset, 0))
}

fn held_scroll_from_bottom(
    scroll_from_bottom: usize,
    old_max_scroll: usize,
    max_scroll: usize,
) -> usize {
    if scroll_from_bottom == 0 {
        return 0;
    }
    let top = old_max_scroll.saturating_sub(scroll_from_bottom);
    max_scroll.saturating_sub(top)
}

#[cfg(test)]
mod tests;
