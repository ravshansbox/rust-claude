use super::{
    App, HistorySearch, Picker, display_model,
    input::{input_cursor, input_rows},
    render::{borrowed_line, theme},
    status::{format_context, format_quota, format_stats},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

const MAX_LIST_ROWS: usize = 10;
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let input_width = frame.area().width.max(1) as usize;
    app.input_width = input_width;
    let input_rows = input_rows(&app.input, input_width);
    let (cursor_row, cursor_column) = input_cursor(&app.input, app.cursor, input_width);
    let input_lines: Vec<Line> = input_rows.into_iter().map(Line::raw).collect();
    let mut footer_parts = vec![
        app.workspace.to_string(),
        format!("{}:{}", display_model(&app.model), app.thinking_level),
        format_context(&app.stats),
        format_stats(&app.stats),
        format_quota(&app.stats),
    ];
    footer_parts.retain(|part| !part.is_empty());
    let footer_paragraph =
        Paragraph::new(Line::raw(footer_parts.join(" · "))).wrap(Wrap { trim: false });
    let footer_height = footer_paragraph
        .line_count(frame.area().width)
        .min(u16::MAX as usize) as u16;
    let [chat, input, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(input_lines.len().min(u16::MAX as usize - 2) as u16 + 2),
            Constraint::Length(footer_height),
        ])
        .areas(frame.area());

    for message in &mut app.messages {
        message.render(chat.width);
    }
    let mut lines = Vec::new();
    for message in &app.messages {
        lines.push(Line::default());
        if let Some((_, rendered)) = &message.rendered {
            lines.extend(rendered.iter().map(borrowed_line));
        }
    }
    if app.busy {
        lines.push(Line::default());
        let frame = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
        lines.push(Line::from(format!("{frame} {}", app.status).dark_gray()));
        for prompt in app.queued_prompts() {
            let first_line = prompt.lines().next().unwrap_or_default();
            lines.push(Line::from(format!("queued: {first_line}").dark_gray()));
        }
    }

    let panel_height =
        |content: usize| (content.min(u16::MAX as usize - 1) as u16 + 1).min(chat.height);
    let panel = if let Some(question) = &app.question {
        let view = question.view();
        let height = panel_height(view.line_count(chat.width));
        Some((view, height, None))
    } else if let Some(search) = &app.history_search {
        let count = search.matches().len();
        let height = panel_height(count.min(MAX_LIST_ROWS) + 2);
        let list = (count, 2, search.selected);
        Some((
            history_view(search, height.saturating_sub(1)),
            height,
            Some(list),
        ))
    } else if let Some(picker) = &app.picker {
        let count = picker.items.len();
        let height = panel_height(count.min(MAX_LIST_ROWS) + 1);
        let list = (count, 1, picker.selected);
        Some((
            picker_view(picker, height.saturating_sub(1)),
            height,
            Some(list),
        ))
    } else {
        None
    };
    let panel_height = panel.as_ref().map_or(0, |(_, height, _)| *height);
    let conversation_area = Rect {
        height: chat.height - panel_height,
        ..chat
    };

    let conversation = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let viewport_height = conversation_area.height;
    let wrapped_line_count = conversation.line_count(chat.width);
    let max_scroll = wrapped_line_count
        .saturating_sub(viewport_height as usize)
        .min(u16::MAX as usize) as u16;
    app.scroll_from_bottom =
        held_scroll_from_bottom(app.scroll_from_bottom, app.max_scroll, max_scroll);
    app.max_scroll = max_scroll;
    app.page_size = viewport_height.max(1);
    let scroll = app.max_scroll.saturating_sub(app.scroll_from_bottom);
    frame.render_widget(conversation.scroll((scroll, 0)), conversation_area);
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
            render_list_scrollbar(frame, area, count, header, selected);
        }
    }

    if let Some(suggestions) = app
        .visible_suggestions()
        .filter(|_| app.history_search.is_none())
    {
        let matches = suggestions.items;
        let height = (matches.len() as u16).min(chat.height);
        let selected = app.command_selected.min(matches.len() - 1);
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
    }

    let input_scroll = (cursor_row as u16).saturating_sub(input.height.saturating_sub(3));
    frame.render_widget(
        Paragraph::new(input_lines)
            .block(Block::default().borders(Borders::TOP | Borders::BOTTOM))
            .scroll((input_scroll, 0)),
        input,
    );
    frame.set_cursor_position((
        input.x + cursor_column as u16,
        input.y + cursor_row as u16 - input_scroll + 1,
    ));

    frame.render_widget(footer_paragraph, footer);
}

fn held_scroll_from_bottom(scroll_from_bottom: u16, old_max_scroll: u16, max_scroll: u16) -> u16 {
    if scroll_from_bottom == 0 {
        return 0;
    }
    let top = old_max_scroll.saturating_sub(scroll_from_bottom);
    max_scroll.saturating_sub(top)
}

fn render_list_scrollbar(
    frame: &mut Frame,
    area: Rect,
    count: usize,
    header: u16,
    selected: usize,
) {
    let rows = area.height.saturating_sub(1 + header);
    let visible = rows as usize;
    if visible == 0 || count <= visible {
        return;
    }
    let list_area = Rect {
        y: area.y + 1 + header,
        height: rows,
        ..area
    };
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
    let tab = |label: &'static str, active: bool| {
        if active {
            Span::from(label).reversed()
        } else {
            Span::from(label)
        }
    };
    let mut lines = vec![
        Line::from(vec![
            Span::from("Prompt history ").bold(),
            tab(" Folder ", !search.all),
            Span::raw(" "),
            tab(" All ", search.all),
            Span::from(" (←→ tab, ↑↓ select, Enter edit, Esc cancel)").bold(),
        ]),
        Line::raw(format!("search: {}", search.query)),
    ];
    let matches = search.matches();
    let visible = (height as usize).saturating_sub(2).max(1);
    let first = (search.selected + 1).saturating_sub(visible);
    for (index, (prompt, folder)) in matches.into_iter().enumerate().skip(first).take(visible) {
        let mut text = prompt.lines().next().unwrap_or_default().to_string();
        if prompt.lines().nth(1).is_some() {
            text.push_str(" …");
        }
        let mut spans = vec![if index == search.selected {
            Span::from(text).reversed()
        } else {
            Span::from(text)
        }];
        if let Some(folder) = folder {
            let name = std::path::Path::new(folder)
                .file_name()
                .map_or(folder.to_string(), |name| {
                    name.to_string_lossy().into_owned()
                });
            spans.push(Span::from(format!("  {name}")).dark_gray());
        }
        lines.push(Line::from(spans));
    }
    Paragraph::new(lines)
}

#[cfg(test)]
mod tests {
    use super::held_scroll_from_bottom;
    use crate::session::SessionSummary;
    use crate::tui::{
        App, UiEvent, handle_agent_event, handle_input,
        test_support::{app_with_reply, press, screen},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn keeps_scrolled_view_still_while_content_grows() {
        assert_eq!(held_scroll_from_bottom(0, 10, 15), 0);
        assert_eq!(held_scroll_from_bottom(1, 15, 20), 6);
        assert_eq!(held_scroll_from_bottom(6, 20, 18), 4);
        assert_eq!(held_scroll_from_bottom(1, 15, 10), 0);
    }

    #[test]
    fn keeps_the_conversation_visible_while_picking_a_thinking_level() {
        let mut app = app_with_reply();
        handle_input(Event::Paste("/thinking".into()), &mut app, |_| {});
        press(&mut app, KeyCode::Enter);
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("Select thinking level"), "{shown}");
    }

    #[test]
    fn keeps_the_conversation_visible_while_picking_a_model() {
        let mut app = app_with_reply();
        handle_agent_event(UiEvent::Models(Ok(vec!["other-model".into()])), &mut app);
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("Select model"), "{shown}");
    }

    #[test]
    fn keeps_the_conversation_visible_while_picking_a_session() {
        let mut app = app_with_reply();
        let session = SessionSummary {
            id: "1".into(),
            modified: std::time::SystemTime::now(),
            preview: "old prompt".into(),
        };
        handle_agent_event(UiEvent::Sessions(Ok(vec![session])), &mut app);
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("Resume session"), "{shown}");
        assert!(shown.contains("old prompt"), "{shown}");
    }

    #[test]
    fn keeps_the_conversation_visible_while_searching_history() {
        let mut app = app_with_reply();
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            &mut app,
            |_| {},
        );
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("Prompt history"), "{shown}");
    }

    fn open_sessions(app: &mut App, count: usize) {
        let sessions = (0..count)
            .map(|index| SessionSummary {
                id: index.to_string(),
                modified: std::time::SystemTime::now(),
                preview: format!("prompt {index:02}"),
            })
            .collect();
        handle_agent_event(UiEvent::Sessions(Ok(sessions)), app);
    }

    #[test]
    fn shows_at_most_ten_items_with_a_scrollbar() {
        let mut app = app_with_reply();
        open_sessions(&mut app, 15);
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("prompt 00"), "{shown}");
        assert!(shown.contains("prompt 09"), "{shown}");
        assert!(!shown.contains("prompt 10"), "{shown}");
        assert!(shown.contains('█'), "{shown}");
        for _ in 0..12 {
            press(&mut app, KeyCode::Down);
        }
        let shown = screen(&mut app);
        assert!(!shown.contains("prompt 02"), "{shown}");
        assert!(shown.contains("prompt 03"), "{shown}");
        assert!(shown.contains("prompt 12"), "{shown}");
        assert!(!shown.contains("prompt 13"), "{shown}");
    }

    #[test]
    fn hides_the_scrollbar_when_items_fit() {
        let mut app = app_with_reply();
        open_sessions(&mut app, 10);
        let shown = screen(&mut app);
        assert!(shown.contains("prompt 09"), "{shown}");
        assert!(!shown.contains('█'), "{shown}");
    }

    #[test]
    fn shows_at_most_ten_history_prompts_with_a_scrollbar() {
        let mut app = app_with_reply();
        app.prompt_history = (0..15).map(|index| format!("prompt {index:02}")).collect();
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            &mut app,
            |_| {},
        );
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains(" Folder "), "{shown}");
        assert!(shown.contains("prompt 14"), "{shown}");
        assert!(shown.contains("prompt 05"), "{shown}");
        assert!(!shown.contains("prompt 04"), "{shown}");
        assert!(shown.contains('█'), "{shown}");
    }
}
