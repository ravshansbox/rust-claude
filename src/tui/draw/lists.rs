use super::saturating_u16;
use crate::tui::{App, HistorySearch, Picker, render::theme};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use views::{history_view, picker_view};

mod views;

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

fn reversed_if(span: Span<'_>, reversed: bool) -> Span<'_> {
    if reversed { span.reversed() } else { span }
}
