use super::{
    App, HistorySearch, Picker, display_model,
    input::{input_cursor, input_rows},
    render::{theme, wrapped_height},
    selection::highlight_selection,
    status::{format_context, format_quota, format_stats},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use std::borrow::Cow;

const MAX_LIST_ROWS: usize = 10;
const BOTTOM_BUTTON: &str = "↓ Bottom (End)";
pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let input_width = frame.area().width.saturating_sub(2).max(1) as usize;
    app.input_width = input_width;
    let input_rows = input_rows(&app.input, input_width);
    let (cursor_row, cursor_column) = input_cursor(&app.input, app.cursor, input_width);
    let input_lines: Vec<Line> = input_rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let marker = if index == 0 { "❯ " } else { "  " };
            Line::raw(format!("{marker}{row}"))
        })
        .collect();
    let footer_paragraph = footer(app);
    let footer_height = footer_paragraph
        .line_count(frame.area().width)
        .min(u16::MAX as usize) as u16;
    let [chat, input, footer] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(input_lines.len().min(u16::MAX as usize - 2) as u16 + 2),
        Constraint::Length(footer_height),
    ])
    .areas(frame.area());

    for message in &mut app.messages {
        message.render(chat.width);
    }

    let panel_height =
        |content: usize| (content.min(u16::MAX as usize - 1) as u16 + 1).min(chat.height);
    let panel = if let Some(picker) = &app.picker {
        let count = picker.items.len();
        let height = panel_height(count.min(MAX_LIST_ROWS) + 1);
        let list = (count, 1, picker.selected);
        Some((
            picker_view(picker, height.saturating_sub(1)),
            height,
            Some(list),
        ))
    } else if let Some(search) = &app.history_search {
        let count = search.matches().len();
        let height = panel_height(count.min(MAX_LIST_ROWS) + 2);
        let list = (count, 2, search.selected);
        Some((
            history_view(search, height.saturating_sub(1)),
            height,
            Some(list),
        ))
    } else {
        None
    };
    let panel_height = panel.as_ref().map_or(0, |(_, height, _)| *height);
    let layout = |app: &App, pinned: bool| {
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
        let pinned_height = pinned_lines
            .iter()
            .map(|line| wrapped_height(line, chat.width))
            .sum::<usize>()
            .min((chat.height - panel_height).saturating_sub(1) as usize)
            as u16;
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
        let max_scroll = wrapped_line_count.saturating_sub(conversation_area.height as usize);
        (pinned_lines, pinned_height, conversation_area, max_scroll)
    };
    let mut pinned = app.scroll_from_bottom > 0;
    let (mut pinned_lines, mut pinned_height, mut conversation_area, mut max_scroll) =
        layout(app, pinned);
    app.scroll_from_bottom = if pinned && !app.status_pinned {
        app.scroll_from_bottom.min(max_scroll)
    } else {
        held_scroll_from_bottom(app.scroll_from_bottom, app.max_scroll, max_scroll)
    };
    if pinned && app.scroll_from_bottom == 0 {
        pinned = false;
        (pinned_lines, pinned_height, conversation_area, max_scroll) = layout(app, false);
    }
    let viewport_height = conversation_area.height;
    app.status_pinned = pinned;
    app.max_scroll = max_scroll;
    app.page_size = viewport_height.max(1) as usize;
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
    app.bottom_button = Rect::default();
    if pinned_height > 0 {
        let width = (Line::raw(BOTTOM_BUTTON).width() as u16).min(chat.width);
        app.bottom_button = Rect {
            x: chat.x + chat.width - width,
            width,
            height: 1,
            ..pinned_area
        };
        frame.render_widget(Clear, app.bottom_button);
        frame.render_widget(Paragraph::new(BOTTOM_BUTTON).dark_gray(), app.bottom_button);
    }
    if let Some((view, height, list)) = panel {
        let area = Rect {
            y: chat.y + chat.height - height,
            height,
            ..chat
        };
        let border = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::new().dark_gray());
        frame.render_widget(Clear, area);
        frame.render_widget(view.block(border), area);
        if let Some((count, header, selected)) = list {
            let list_area = Rect {
                y: area.y + 1 + header,
                height: area.height.saturating_sub(1 + header),
                ..area
            };
            render_list_scrollbar(frame, list_area, count, selected);
        }
    }

    draw_suggestions(frame, app, chat);

    let input_scroll = (cursor_row as u16).saturating_sub(input.height.saturating_sub(3));
    frame.render_widget(
        Paragraph::new(input_lines)
            .block(Block::default().borders(Borders::TOP | Borders::BOTTOM))
            .scroll((input_scroll, 0)),
        input,
    );
    frame.set_cursor_position((
        input.x + 2 + cursor_column as u16,
        input.y + cursor_row as u16 - input_scroll + 1,
    ));

    frame.render_widget(footer_paragraph, footer);
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

fn draw_suggestions(frame: &mut Frame, app: &App, chat: Rect) {
    let Some(suggestions) = app
        .visible_suggestions()
        .filter(|_| app.history_search.is_none() && app.picker.is_none())
    else {
        return;
    };
    let matches = suggestions.items;
    let height = (matches.len().min(MAX_LIST_ROWS) as u16).min(chat.height);
    let selected = app.command_selected.min(matches.len() - 1);
    let first = (selected + 1).saturating_sub(height as usize);
    let name_width = matches
        .iter()
        .map(|(name, _)| name.chars().count() + 1)
        .max()
        .unwrap_or_default()
        .max(12);
    let texts: Vec<String> = matches
        .iter()
        .map(|(name, description)| format!(" {name:<name_width$}{description} "))
        .collect();
    let width = texts
        .iter()
        .map(|text| Line::raw(text.as_str()).width())
        .max()
        .unwrap_or_default();
    let lines: Vec<Line> = texts
        .into_iter()
        .enumerate()
        .skip(first)
        .take(height as usize)
        .map(|(index, text)| {
            let text = format!("{text:<width$}");
            if index == selected {
                Line::from(text.reversed())
            } else {
                Line::raw(text)
            }
        })
        .collect();
    let width = width.min(chat.width as usize) as u16;
    let area = Rect {
        y: chat.y + chat.height - height,
        height,
        width,
        ..chat
    };
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).style(theme().subtle_style()), area);
    render_list_scrollbar(frame, area, matches.len(), selected);
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
        if filled >= top - skipped + height_shown as usize {
            break;
        }
        filled += height;
        lines.push(line);
    }
    let offset = (top - skipped).min(u16::MAX as usize) as u16;
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

fn render_list_scrollbar(frame: &mut Frame, list_area: Rect, count: usize, selected: usize) {
    let visible = list_area.height as usize;
    if visible == 0 || count <= visible {
        return;
    }
    let mut state = ScrollbarState::new(count - visible + 1)
        .viewport_content_length(visible)
        .position((selected + 1).saturating_sub(visible));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None);
    frame.render_stateful_widget(scrollbar, list_area, &mut state);
}

fn picker_view(picker: &Picker, height: u16) -> Paragraph<'_> {
    let mut lines = vec![Line::from(
        format!("{} (↑↓ select, Enter confirm, Esc cancel)", picker.title).bold(),
    )];
    let visible = (height as usize).saturating_sub(1).max(1);
    let first = (picker.selected + 1).saturating_sub(visible);
    for (index, (_, text)) in picker.items.iter().enumerate().skip(first).take(visible) {
        lines.push(if index == picker.selected {
            Line::from(text.as_str().reversed())
        } else {
            Line::raw(text.as_str())
        });
    }
    Paragraph::new(lines)
}

fn history_view(search: &HistorySearch, height: u16) -> Paragraph<'_> {
    let mut lines = vec![
        Line::from(vec![
            Span::from("Prompt history ").bold(),
            reversed_if(Span::from(" Folder "), !search.all),
            Span::raw(" "),
            reversed_if(Span::from(" All "), search.all),
            Span::from(" (←→ tab, ↑↓ select, Enter edit, Esc cancel)").bold(),
        ]),
        Line::raw(format!("search: {}", search.query)),
    ];
    let matches = search.matches();
    let visible = (height as usize).saturating_sub(2).max(1);
    let first = (search.selected + 1).saturating_sub(visible);
    for (index, (prompt, folder)) in matches.into_iter().enumerate().skip(first).take(visible) {
        let mut prompt_lines = prompt.lines();
        let first_line = prompt_lines.next().unwrap_or_default();
        let text: Cow<str> = if prompt_lines.next().is_some() {
            format!("{first_line} …").into()
        } else {
            first_line.into()
        };
        let mut spans = vec![reversed_if(Span::from(text), index == search.selected)];
        if let Some(folder) = folder {
            let name = std::path::Path::new(folder)
                .file_name()
                .map_or(Cow::Borrowed(folder), |name| name.to_string_lossy());
            spans.push(Span::from(format!("  {name}")).dark_gray());
        }
        lines.push(Line::from(spans));
    }
    Paragraph::new(lines)
}

fn reversed_if(span: Span<'_>, reversed: bool) -> Span<'_> {
    if reversed { span.reversed() } else { span }
}

#[cfg(test)]
mod tests;
