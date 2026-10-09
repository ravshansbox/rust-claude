mod app;
mod commands;
mod files;
mod input;
mod keys;
mod question;
mod render;
mod replay;
mod status;
#[cfg(test)]
mod test_support;
mod worker;

use crate::{agent::Agent, clipboard, history, images, settings::Settings, skills::Scope};
use anyhow::Result;
use app::{App, HistorySearch, Picker, PickerKind, Role};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        EventStream, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute,
};
use futures::StreamExt;
use input::{input_cursor, input_rows};
use keys::{Action, handle_input};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use render::{THEME, Theme, borrowed_line, theme};
use replay::{handle_agent_event, replay_messages};
use status::{format_context, format_quota, format_stats};
use std::time::{Duration, SystemTime};
use tokio::{sync::mpsc, time::MissedTickBehavior};
use worker::{Request, UiEvent, agent_task};

pub fn dark_theme() -> bool {
    matches!(Theme::detect(), Theme::Dark)
}

const MAX_LIST_ROWS: usize = 10;
const REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub async fn run(agent: Agent) -> Result<()> {
    THEME.get_or_init(Theme::detect);
    let mut terminal = ratatui::init();
    if let Err(error) = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste) {
        ratatui::restore();
        return Err(error.into());
    }
    let keyboard_enhanced = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();

    let result = run_loop(&mut terminal, agent).await;
    if keyboard_enhanced {
        let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    }
    let mouse_result = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
    ratatui::restore();

    mouse_result?;
    result
}

async fn run_loop(terminal: &mut DefaultTerminal, mut agent: Agent) -> Result<()> {
    agent.ask_user = true;
    let mut app = App::new(&agent.model, agent.thinking_level, agent.stats());
    for instructions in &agent.instructions {
        app.push(Role::Event, format!("loaded {}", instructions.label));
    }
    for scope in [Scope::Global, Scope::Project] {
        let names: Vec<&str> = agent
            .skills
            .skills
            .iter()
            .filter(|skill| skill.scope == scope)
            .map(|skill| skill.name.as_str())
            .collect();
        if !names.is_empty() {
            app.push(
                Role::Event,
                format!("loaded {scope} skills: {}", names.join(", ")),
            );
        }
    }
    for diagnostic in &agent.skills.diagnostics {
        app.push(Role::Event, diagnostic.to_string());
    }
    app.skills = agent.skills.skills.clone();
    for loaded in agent.mcp.loaded() {
        app.push(Role::Event, loaded);
    }
    for diagnostic in &agent.mcp.diagnostics {
        app.push(Role::Event, diagnostic.to_string());
    }
    for program in &agent.missing_programs {
        app.push(Role::Event, format!("{program} not found on PATH"));
    }
    app.push(
        Role::Event,
        "Ask me to inspect, explain, or edit this project.",
    );
    if !agent.messages().is_empty() {
        replay_messages(&mut app, agent.messages());
        app.push(Role::Event, "continued session");
    }
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let image_events = event_tx.clone();
    app.queue = agent.queue.clone();
    app.history_file = history::history_path();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, event_tx));
    let mut terminal_events = EventStream::new();
    let mut redraw = tokio::time::interval(REDRAW_INTERVAL);
    redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut spinner = tokio::time::interval(SPINNER_INTERVAL);
    spinner.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;
    let mut saved_model = app.model.clone();
    let mut saved_thinking_level = app.thinking_level;

    let result = loop {
        tokio::select! {
            _ = redraw.tick(), if dirty => {
                terminal.draw(|frame| draw(frame, &mut app))?;
                dirty = false;
            }
            _ = spinner.tick(), if app.busy => {
                app.spinner_frame = app.spinner_frame.wrapping_add(1);
                dirty = true;
            }
            event = terminal_events.next() => {
                dirty = true;
                let event = match event {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => break Err(error.into()),
                    None => break Ok(()),
                };
                let quit = handle_input(event, &mut app, |action| match action {
                    Action::Submit(prompt, images, thinking_level) => {
                        let _ = request_tx.send(Request::Prompt(prompt, images, thinking_level));
                    }
                    Action::Shell(command) => {
                        let _ = request_tx.send(Request::Shell(command));
                    }
                    Action::PasteImage => {
                        let events = image_events.clone();
                        tokio::task::spawn_blocking(move || {
                            let result = clipboard::read_image()
                                .and_then(|data| data.map(images::prepare).transpose());
                            let _ = events.send(UiEvent::ImagePasted(result));
                        });
                    }
                    Action::Compact(thinking_level) => {
                        let _ = request_tx.send(Request::Compact(thinking_level));
                    }
                    Action::Context => {
                        let _ = request_tx.send(Request::Context);
                    }
                    Action::ListSessions => {
                        let _ = request_tx.send(Request::ListSessions);
                    }
                    Action::Resume(id) => {
                        let _ = request_tx.send(Request::Resume(id));
                    }
                    Action::SetModel(model) => {
                        let _ = request_tx.send(Request::SetModel(model));
                    }
                    Action::CheckModel(model) => {
                        let _ = request_tx.send(Request::CheckModel(model));
                    }
                    Action::NewSession => {
                        let _ = request_tx.send(Request::NewSession);
                    }
                    Action::ListModels => {
                        let _ = request_tx.send(Request::ListModels);
                    }
                    Action::Cancel => {
                        let _ = cancel_tx.send(());
                    }
                });
                save_changed_settings(&mut app, &mut saved_model, &mut saved_thinking_level);
                if quit {
                    break Ok(());
                }
            }
            Some(event) = event_rx.recv() => {
                dirty = true;
                let mut next = Some(event);
                while let Some(event) = next {
                    handle_agent_event(event, &mut app);
                    next = event_rx.try_recv().ok();
                }
                if !app.busy
                    && let Some((prompt, images)) = app.send_queued()
                {
                    let _ = request_tx.send(Request::Prompt(prompt, images, app.thinking_level));
                }
                save_changed_settings(&mut app, &mut saved_model, &mut saved_thinking_level);
            }
        }
    };
    drop(request_tx);
    let _ = cancel_tx.send(());
    let _ = worker.await;
    result
}

fn save_changed_settings(
    app: &mut App,
    saved_model: &mut String,
    saved_thinking_level: &mut &'static str,
) {
    if app.model == *saved_model && app.thinking_level == *saved_thinking_level {
        return;
    }
    let mut settings = Settings::load();
    if app.model != *saved_model {
        *saved_model = app.model.clone();
        settings.model = Some(saved_model.clone());
    }
    if app.thinking_level != *saved_thinking_level {
        *saved_thinking_level = app.thinking_level;
        settings.thinking_level = Some(saved_thinking_level.to_string());
    }
    if let Err(error) = settings.save() {
        app.push(Role::Event, format!("failed to save settings: {error}"));
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
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
            tab(" Current ", !search.all),
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

fn time_ago(time: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(time)
        .unwrap_or_default()
        .as_secs();
    match seconds {
        0..60 => format!("{seconds}s ago"),
        60..3_600 => format!("{}m ago", seconds / 60),
        3_600..86_400 => format!("{}h ago", seconds / 3_600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

fn workspace_label() -> String {
    let folder = std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|branch| !branch.is_empty());
    match branch {
        Some(branch) => format!("{folder} · {branch}"),
        None => folder,
    }
}

fn display_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

#[cfg(test)]
mod tests {
    use super::test_support::{new_app, press, type_text};
    use super::{
        App, Role, UiEvent, draw, handle_agent_event, handle_input, held_scroll_from_bottom,
    };
    use crate::agent::AgentEvent;
    use crate::ask;
    use crate::session::SessionSummary;
    use crate::tui::files::{file_matches, file_query};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use ratatui::{Terminal, backend::TestBackend};
    use serde_json::json;
    use tokio::sync::oneshot::{self, error::TryRecvError};

    #[test]
    fn finds_file_query_at_cursor() {
        assert_eq!(file_query("read @src/ma", 12), Some((5, "src/ma")));
        assert_eq!(file_query("@", 1), Some((0, "")));
        assert_eq!(file_query("mail@host", 9), None);
        assert_eq!(file_query("@src now", 8), None);
    }

    #[test]
    fn matches_files_ignoring_case() {
        let files = vec!["README.md".to_string(), "src/main.rs".to_string()];
        assert_eq!(file_matches(&files, "readme"), vec!["README.md"]);
        assert_eq!(file_matches(&files, "").len(), 2);
    }

    #[test]
    fn keeps_scrolled_view_still_while_content_grows() {
        assert_eq!(held_scroll_from_bottom(0, 10, 15), 0);
        assert_eq!(held_scroll_from_bottom(1, 15, 20), 6);
        assert_eq!(held_scroll_from_bottom(6, 20, 18), 4);
        assert_eq!(held_scroll_from_bottom(1, 15, 10), 0);
    }

    fn screen(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn ask(app: &mut App, input: serde_json::Value) -> oneshot::Receiver<Vec<Vec<String>>> {
        let (reply, answers) = oneshot::channel();
        app.busy = true;
        handle_agent_event(
            UiEvent::Agent(AgentEvent::Question {
                questions: ask::parse(&input).unwrap(),
                reply,
            }),
            app,
        );
        answers
    }

    fn output_question(multi_select: bool) -> serde_json::Value {
        json!({ "questions": [{
            "question": "Which output?",
            "header": "Output",
            "multi_select": multi_select,
            "options": [
                { "label": "JSON", "description": "Structured", "recommended": true },
                { "label": "Text", "description": "Readable" }
            ]
        }] })
    }

    #[test]
    fn shows_a_question_and_sends_the_chosen_option() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        let shown = screen(&mut app);
        assert!(shown.contains("Output: Which output?"), "{shown}");
        assert!(shown.contains("JSON (recommended): Structured"), "{shown}");
        assert!(shown.contains("Text: Readable"), "{shown}");
        assert!(shown.contains("Other: "), "{shown}");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv().unwrap(), vec![vec!["Text".to_string()]]);
        assert!(!screen(&mut app).contains("Which output?"));
    }

    #[test]
    fn sends_a_typed_answer_for_other() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        type_text(&mut app, "YAML please");
        assert!(screen(&mut app).contains("Other: YAML please"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["Other: YAML please".to_string()]]
        );
        assert_eq!(app.input, "");
    }

    #[test]
    fn ignores_enter_on_an_empty_other() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Empty));
        assert!(screen(&mut app).contains("Which output?"));
    }

    #[test]
    fn toggles_options_in_a_multiple_choice_question() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(true));
        press(&mut app, KeyCode::Char(' '));
        assert!(screen(&mut app).contains("[x] JSON"));
        assert!(screen(&mut app).contains("[ ] Text"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        type_text(&mut app, "a b");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["JSON".to_string(), "Other: a b".to_string()]]
        );
    }

    #[test]
    fn asks_each_question_in_turn() {
        let mut app = new_app();
        let mut input = output_question(false);
        let mut second = input["questions"][0].clone();
        second["question"] = json!("Which colour?");
        second["header"] = json!("Colour");
        input["questions"].as_array_mut().unwrap().push(second);
        let mut answers = ask(&mut app, input);
        assert!(screen(&mut app).contains("Output (1/2): Which output?"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Empty));
        assert!(screen(&mut app).contains("Colour (2/2): Which colour?"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["JSON".to_string()], vec!["Text".to_string()]]
        );
    }

    #[test]
    fn declines_the_question_on_escape() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Esc);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Closed));
        assert!(!screen(&mut app).contains("Which output?"));
        assert!(app.busy);
    }

    fn app_with_reply() -> App {
        let mut app = new_app();
        app.push(Role::Assistant, "Earlier reply");
        app
    }

    #[test]
    fn keeps_the_conversation_visible_while_asking() {
        let mut app = app_with_reply();
        ask(&mut app, output_question(false));
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("Which output?"), "{shown}");
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
        assert!(shown.contains("prompt 14"), "{shown}");
        assert!(shown.contains("prompt 05"), "{shown}");
        assert!(!shown.contains("prompt 04"), "{shown}");
        assert!(shown.contains('█'), "{shown}");
    }

    #[test]
    fn highlights_the_recommended_option_even_when_listed_later() {
        let mut app = new_app();
        let mut answers = ask(
            &mut app,
            json!({ "questions": [{
                "question": "Which output?",
                "header": "Output",
                "options": [
                    { "label": "Text", "description": "Readable" },
                    { "label": "JSON", "description": "Structured", "recommended": true }
                ]
            }] }),
        );
        let shown = screen(&mut app);
        let json_row = shown.find("JSON (recommended)").unwrap();
        let text_row = shown.find("Text: Readable").unwrap();
        assert!(json_row < text_row, "{shown}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv().unwrap(), vec![vec!["JSON".to_string()]]);
    }
}
