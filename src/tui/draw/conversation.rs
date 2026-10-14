use super::{SPINNER_FRAMES, saturating_u16};
use crate::tui::{App, render::wrapped_height};
use ratatui::{
    layout::Rect,
    style::Stylize,
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
};

pub(super) fn conversation_layout(
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
pub(in crate::tui) fn conversation(
    app: &App,
    width: u16,
    top: usize,
    height_shown: u16,
) -> Paragraph<'_> {
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

pub(super) fn held_scroll_from_bottom(
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
