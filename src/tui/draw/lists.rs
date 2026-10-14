use super::saturating_u16;
use crate::tui::{App, HistorySearch, Picker, render::theme};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use std::borrow::Cow;

const MAX_LIST_ROWS: usize = 10;

pub(super) type Panel<'a> = (Paragraph<'a>, u16, Option<(usize, u16, usize)>);

pub(super) fn panel<'a>(
    picker: Option<&'a Picker>,
    history_search: Option<&'a HistorySearch>,
    chat: Rect,
) -> Option<Panel<'a>> {
    let panel_height = |content: usize| saturating_u16(content).saturating_add(1).min(chat.height);
    if let Some(picker) = picker {
        let count = picker.items.len();
        let height = panel_height(count.min(MAX_LIST_ROWS) + 1);
        let list = (count, 1, picker.selected);
        Some((
            picker_view(picker, height.saturating_sub(1)),
            height,
            Some(list),
        ))
    } else if let Some(search) = history_search {
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
    }
}

pub(super) fn draw_panel(frame: &mut Frame, chat: Rect, panel: Option<Panel>) {
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
}

pub(super) fn draw_suggestions(frame: &mut Frame, app: &App, chat: Rect) {
    let Some(suggestions) = app
        .visible_suggestions()
        .filter(|_| app.history_search.is_none() && app.picker.is_none())
    else {
        return;
    };
    let matches = suggestions.items;
    let height = saturating_u16(matches.len().min(MAX_LIST_ROWS)).min(chat.height);
    let selected = app.command_selected.min(matches.len() - 1);
    let first = (selected + 1).saturating_sub(usize::from(height));
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
        .take(usize::from(height))
        .map(|(index, text)| {
            Line::from(reversed_if(
                Span::from(format!("{text:<width$}")),
                index == selected,
            ))
        })
        .collect();
    let width = saturating_u16(width).min(chat.width);
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

fn render_list_scrollbar(frame: &mut Frame, list_area: Rect, count: usize, selected: usize) {
    let visible = usize::from(list_area.height);
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
    let visible = usize::from(height).saturating_sub(1).max(1);
    let first = (picker.selected + 1).saturating_sub(visible);
    for (index, (_, text)) in picker.items.iter().enumerate().skip(first).take(visible) {
        lines.push(Line::from(reversed_if(
            Span::from(text.as_str()),
            index == picker.selected,
        )));
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
    let visible = usize::from(height).saturating_sub(2).max(1);
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
