use super::{
    App, HistorySearch, Picker, display_model,
    input::{input_cursor, input_rows},
    render::{borrowed_line, theme, wrapped_height},
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
pub(super) const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    let mut status_lines = Vec::new();
    if let Some(activity) = app.activity {
        status_lines.push(Line::default());
        let frame = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
        status_lines.push(Line::from(format!("{frame} {activity}").dark_gray()));
        for prompt in app.queued_prompts() {
            let first_line = prompt.lines().next().unwrap_or_default();
            status_lines.push(Line::from(format!("queued: {first_line}").dark_gray()));
        }
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
    let conversation_area = Rect {
        height: chat.height - panel_height,
        ..chat
    };

    let wrap = Wrap { trim: false };
    let status_heights: Vec<usize> = status_lines
        .iter()
        .map(|line| wrapped_height(line, chat.width))
        .collect();
    // Each message starts with a blank line, one row high.
    let wrapped_line_count: usize = app
        .messages
        .iter()
        .map(|message| 1 + message.height())
        .sum::<usize>()
        + status_heights.iter().sum::<usize>();
    let viewport_height = conversation_area.height;
    let max_scroll = wrapped_line_count.saturating_sub(viewport_height as usize);
    app.scroll_from_bottom =
        held_scroll_from_bottom(app.scroll_from_bottom, app.max_scroll, max_scroll);
    app.max_scroll = max_scroll;
    app.page_size = viewport_height.max(1) as usize;
    let scroll = app.max_scroll.saturating_sub(app.scroll_from_bottom);
    // Paragraph offsets are u16, so drop the lines above the view and scroll
    // only within the first visible one. Lines below the view are left out.
    let mut skipped = 0;
    let mut first_message = 0;
    while let Some(message) = app.messages.get(first_message)
        && skipped + 1 + message.height() <= scroll
    {
        skipped += 1 + message.height();
        first_message += 1;
    }
    let rows = app.messages[first_message..]
        .iter()
        .flat_map(|message| {
            let rendered = message.rendered.iter().flat_map(|rendered| {
                rendered
                    .lines
                    .iter()
                    .map(borrowed_line)
                    .zip(rendered.heights.iter().copied())
            });
            std::iter::once((Line::default(), 1)).chain(rendered)
        })
        .chain(status_lines.into_iter().zip(status_heights));
    let mut lines = Vec::new();
    let mut filled = 0;
    for (line, height) in rows {
        if lines.is_empty() && skipped + height <= scroll {
            skipped += height;
            continue;
        }
        if filled >= scroll - skipped + viewport_height as usize {
            break;
        }
        filled += height;
        lines.push(line);
    }
    let offset = (scroll - skipped).min(u16::MAX as usize) as u16;
    let conversation = Paragraph::new(Text::from(lines)).wrap(wrap);
    frame.render_widget(conversation.scroll((offset, 0)), conversation_area);
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

    if let Some(suggestions) = app
        .visible_suggestions()
        .filter(|_| app.history_search.is_none() && app.picker.is_none())
    {
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
    use crate::agent::AgentEvent;
    use crate::session::SessionSummary;
    use crate::skills::{Scope, Skill};
    use crate::tui::{
        App, Role, UiEvent, handle_agent_event, handle_input,
        test_support::{app_with_reply, press, screen},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn keeps_scrolled_view_still_while_content_grows() {
        assert_eq!(held_scroll_from_bottom(0, 10, 15), 0);
        assert_eq!(held_scroll_from_bottom(1, 15, 20), 6);
        assert_eq!(held_scroll_from_bottom(6, 20, 18), 4);
        assert_eq!(held_scroll_from_bottom(1, 15, 10), 0);
    }

    #[test]
    fn shows_the_end_of_a_conversation_longer_than_65535_lines() {
        let mut app = app_with_reply();
        for number in 0..40_000 {
            app.push(Role::Event, format!("event {number}"));
        }
        let shown = screen(&mut app);
        assert!(shown.contains("event 39999"), "{shown}");
        press(&mut app, KeyCode::Home);
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        press(&mut app, KeyCode::PageDown);
        let shown = screen(&mut app);
        assert!(shown.contains("event 10"), "{shown}");
    }

    fn screen_of_width(app: &mut App, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|frame| super::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn follows_the_end_after_resizing_and_while_a_reply_grows() {
        let mut app = app_with_reply();
        let words: String = (0..100).map(|number| format!("word{number:02} ")).collect();
        app.push(Role::Event, format!("{words}END"));
        let shown = screen_of_width(&mut app, 100);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("END"), "{shown}");
        let shown = screen_of_width(&mut app, 30);
        assert!(!shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("END"), "{shown}");
        press(&mut app, KeyCode::Home);
        let shown = screen_of_width(&mut app, 30);
        assert!(shown.contains("Earlier reply"), "{shown}");
        press(&mut app, KeyCode::End);
        handle_agent_event(UiEvent::Agent(AgentEvent::Text("Start".into())), &mut app);
        let shown = screen_of_width(&mut app, 30);
        assert!(shown.contains("Start"), "{shown}");
        let more: String = (0..30).map(|number| format!("\n\nline {number}")).collect();
        handle_agent_event(UiEvent::Agent(AgentEvent::Text(more)), &mut app);
        let shown = screen_of_width(&mut app, 30);
        assert!(shown.contains("line 29"), "{shown}");
        assert!(!shown.contains("Start"), "{shown}");
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
        handle_agent_event(
            UiEvent::Models(Ok(vec!["claude-opus-5-5".into()])),
            &mut app,
        );
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

    #[test]
    fn keeps_the_selected_command_visible_in_a_long_list() {
        let mut app = app_with_reply();
        app.skills = (0..30)
            .map(|index| Skill {
                name: format!("skill{index:02}"),
                description: format!("skill number {index}"),
                path: "/skills/SKILL.md".into(),
                base_dir: "/skills".into(),
                disable_model_invocation: false,
                scope: Scope::Global,
            })
            .collect();
        handle_input(Event::Paste("/".into()), &mut app, |_| {});
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("/new"), "{shown}");
        assert!(!shown.contains("/skill:skill03"), "{shown}");
        assert!(shown.contains('█'), "{shown}");
        for _ in 0..36 {
            press(&mut app, KeyCode::Down);
        }
        let shown = screen(&mut app);
        assert!(shown.contains("/skill:skill29"), "{shown}");
        assert!(shown.contains("/skill:skill20"), "{shown}");
        assert!(!shown.contains("/skill:skill19"), "{shown}");
    }
}
