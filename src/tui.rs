use crate::{
    agent::{Agent, AgentEvent, THINKING_LEVELS},
    session::SessionSummary,
    tools,
};
use anyhow::Result;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
};
use futures::StreamExt;
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Margin},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::Value;
use std::time::{Duration, SystemTime};
use tokio::{sync::mpsc, time::MissedTickBehavior};

const REDRAW_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Clone, Copy)]
enum Role {
    User,
    Assistant,
    Thinking,
    Tool,
    Event,
}

struct ChatMessage {
    role: Role,
    text: String,
    rendered: Option<(u16, Vec<Line<'static>>)>,
}

impl ChatMessage {
    fn append(&mut self, text: &str) {
        self.text.push_str(text);
        self.rendered = None;
    }

    fn render(&mut self, width: u16) {
        if self
            .rendered
            .as_ref()
            .is_none_or(|(rendered_width, _)| *rendered_width != width)
        {
            self.rendered = Some((width, render_message(self.role, &self.text, width)));
        }
    }
}

fn render_message(role: Role, text: &str, width: u16) -> Vec<Line<'static>> {
    match role {
        Role::User => user_message_lines(text, width as usize),
        Role::Assistant => tui_markdown::from_str(text)
            .lines
            .into_iter()
            .map(owned_line)
            .collect(),
        Role::Thinking => text
            .lines()
            .map(|line| Line::from(line.to_string().dark_gray().italic()))
            .collect(),
        Role::Tool | Role::Event => text
            .lines()
            .map(|line| Line::raw(line.to_string()))
            .collect(),
    }
}

fn owned_line(line: Line<'_>) -> Line<'static> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .into_iter()
            .map(|span| Span::styled(span.content.into_owned(), span.style))
            .collect(),
    }
}

fn borrowed_line<'a>(line: &'a Line<'static>) -> Line<'a> {
    Line {
        style: line.style,
        alignment: line.alignment,
        spans: line
            .spans
            .iter()
            .map(|span| Span::styled(span.content.as_ref(), span.style))
            .collect(),
    }
}

struct App {
    input: String,
    messages: Vec<ChatMessage>,
    model: String,
    thinking_level: &'static str,
    status: String,
    usage: String,
    scroll_from_bottom: u16,
    max_scroll: u16,
    page_size: u16,
    busy: bool,
    picker: Option<Picker>,
}

struct Picker {
    sessions: Vec<SessionSummary>,
    selected: usize,
}

impl App {
    fn new(model: &str, thinking_level: &'static str) -> Self {
        let mut app = Self {
            input: String::new(),
            messages: Vec::new(),
            model: model.into(),
            thinking_level,
            status: String::new(),
            usage: String::new(),
            scroll_from_bottom: 0,
            max_scroll: 0,
            page_size: 1,
            busy: false,
            picker: None,
        };
        app.push(
            Role::Event,
            "Ask me to inspect, explain, or edit this project.",
        );
        app
    }

    fn push(&mut self, role: Role, text: impl Into<String>) {
        self.messages.push(ChatMessage {
            role,
            text: text.into(),
            rendered: None,
        });
    }

    fn scroll_up(&mut self, amount: u16) {
        self.scroll_from_bottom = self
            .scroll_from_bottom
            .saturating_add(amount)
            .min(self.max_scroll);
    }

    fn scroll_down(&mut self, amount: u16) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(amount);
    }

    fn scroll_to_top(&mut self) {
        self.scroll_from_bottom = self.max_scroll;
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }

    fn cycle_thinking_level(&mut self) {
        let index = THINKING_LEVELS
            .iter()
            .position(|level| *level == self.thinking_level)
            .map_or(0, |index| (index + 1) % THINKING_LEVELS.len());
        self.thinking_level = THINKING_LEVELS[index];
    }
}

pub async fn run(agent: Agent) -> Result<()> {
    let mut terminal = ratatui::init();
    if let Err(error) = execute!(std::io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(error.into());
    }

    let result = run_loop(&mut terminal, agent).await;
    let mouse_result = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();

    mouse_result?;
    result
}

enum UiEvent {
    Agent(AgentEvent),
    Done(Result<()>),
    Cancelled,
    Sessions(Result<Vec<SessionSummary>>),
    Resumed(Result<Vec<Value>>),
}

enum Request {
    Prompt(String, &'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
}

async fn agent_task(
    mut agent: Agent,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut cancel: mpsc::UnboundedReceiver<()>,
    events: mpsc::UnboundedSender<UiEvent>,
) {
    while let Some(request) = requests.recv().await {
        let (prompt, thinking_level) = match request {
            Request::Prompt(prompt, thinking_level) => (prompt, thinking_level),
            Request::ListSessions => {
                let _ = events.send(UiEvent::Sessions(agent.list_sessions()));
                continue;
            }
            Request::Resume(id) => {
                let _ = events.send(UiEvent::Resumed(agent.resume(&id)));
                continue;
            }
            Request::SetModel(model) => {
                agent.model = model;
                continue;
            }
        };
        agent.thinking_level = thinking_level;
        while cancel.try_recv().is_ok() {}

        let checkpoint = agent.history_len();
        let cancelled = {
            let run = agent.prompt(&prompt, |event| {
                let _ = events.send(UiEvent::Agent(event));
            });
            tokio::select! {
                result = run => {
                    let _ = events.send(UiEvent::Done(result));
                    false
                }
                _ = cancel.recv() => true,
            }
        };
        if cancelled {
            agent.rollback(checkpoint);
            let _ = events.send(UiEvent::Cancelled);
        }
    }
}

async fn run_loop(terminal: &mut DefaultTerminal, agent: Agent) -> Result<()> {
    let mut app = App::new(&agent.model, agent.thinking_level);
    for instructions in &agent.instructions {
        app.push(Role::Event, format!("loaded {}", instructions.label));
    }
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, event_tx));
    let mut terminal_events = EventStream::new();
    let mut redraw = tokio::time::interval(REDRAW_INTERVAL);
    redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;

    let result = loop {
        tokio::select! {
            _ = redraw.tick(), if dirty => {
                terminal.draw(|frame| draw(frame, &mut app))?;
                dirty = false;
            }
            event = terminal_events.next() => {
                dirty = true;
                let event = match event {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => break Err(error.into()),
                    None => break Ok(()),
                };
                let quit = handle_input(event, &mut app, |action| match action {
                    Action::Submit(prompt, thinking_level) => {
                        let _ = request_tx.send(Request::Prompt(prompt, thinking_level));
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
                    Action::Cancel => {
                        let _ = cancel_tx.send(());
                    }
                });
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
            }
        }
    };
    worker.abort();
    result
}

enum Action {
    Submit(String, &'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    Cancel,
}

fn handle_input(event: Event, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    if let Event::Mouse(mouse) = event {
        match mouse.kind {
            MouseEventKind::ScrollUp => app.scroll_up(3),
            MouseEventKind::ScrollDown => app.scroll_down(3),
            _ => {}
        }
        return false;
    }
    let Event::Key(key) = event else { return false };
    if key.kind != KeyEventKind::Press {
        return false;
    }

    if let Some(picker) = &mut app.picker {
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => picker.selected = (picker.selected + 1).min(picker.sessions.len() - 1),
            KeyCode::Enter => {
                let id = picker.sessions[picker.selected].id.clone();
                app.picker = None;
                app.status = "resuming".into();
                app.busy = true;
                act(Action::Resume(id));
            }
            KeyCode::Esc => app.picker = None,
            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => return true,
            _ => {}
        }
        return false;
    }

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
        (KeyCode::Esc, _) if app.busy => {
            app.status = "cancelling".into();
            act(Action::Cancel);
        }
        (KeyCode::Esc, _) => return true,
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => return true,
        (KeyCode::BackTab, _) => app.cycle_thinking_level(),
        (KeyCode::Up, _) => app.scroll_up(1),
        (KeyCode::Down, _) => app.scroll_down(1),
        (KeyCode::PageUp, _) => app.scroll_up(app.page_size),
        (KeyCode::PageDown, _) => app.scroll_down(app.page_size),
        (KeyCode::Home, _) => app.scroll_to_top(),
        (KeyCode::End, _) => app.scroll_to_bottom(),
        (KeyCode::Enter, _) if !app.busy && !app.input.trim().is_empty() => {
            app.scroll_to_bottom();
            let prompt = std::mem::take(&mut app.input);
            if prompt.starts_with('/') {
                let (command, argument) = prompt
                    .trim()
                    .split_once(char::is_whitespace)
                    .map_or((prompt.trim(), ""), |(command, argument)| {
                        (command, argument.trim())
                    });
                match command {
                    "/quit" => return true,
                    "/thinking" if argument.is_empty() => app.push(
                        Role::Event,
                        format!(
                            "thinking: {} (options: {})",
                            app.thinking_level,
                            THINKING_LEVELS.join(", ")
                        ),
                    ),
                    "/thinking" => match THINKING_LEVELS.iter().find(|level| **level == argument) {
                        Some(level) => {
                            app.thinking_level = level;
                            app.push(Role::Event, format!("thinking: {level}"));
                        }
                        None => app.push(
                            Role::Event,
                            format!(
                                "unknown thinking level: {argument} (options: {})",
                                THINKING_LEVELS.join(", ")
                            ),
                        ),
                    },
                    "/model" if argument.is_empty() => {
                        app.push(Role::Event, format!("model: {}", app.model));
                    }
                    "/model" => {
                        app.model = argument.to_string();
                        app.push(Role::Event, format!("model: {argument}"));
                        act(Action::SetModel(argument.to_string()));
                    }
                    "/resume" => {
                        app.status = "loading sessions".into();
                        app.busy = true;
                        act(Action::ListSessions);
                    }
                    command => app.push(Role::Event, format!("unknown command: {command}")),
                }
                return false;
            }
            app.push(Role::User, prompt.clone());
            app.status = "thinking".into();
            app.busy = true;
            act(Action::Submit(prompt, app.thinking_level));
        }
        (KeyCode::Backspace, _) => {
            app.input.pop();
        }
        (KeyCode::Char(character), _) => app.input.push(character),
        _ => {}
    }
    false
}

fn handle_agent_event(event: UiEvent, app: &mut App) {
    match event {
        UiEvent::Agent(AgentEvent::Text(text)) => match app.messages.last_mut() {
            Some(last) if matches!(last.role, Role::Assistant) => last.append(&text),
            _ => app.push(Role::Assistant, text),
        },
        UiEvent::Agent(AgentEvent::Thinking(text)) => match app.messages.last_mut() {
            Some(last) if matches!(last.role, Role::Thinking) => last.append(&text),
            _ => app.push(Role::Thinking, text),
        },
        UiEvent::Agent(AgentEvent::ToolStart { name, summary }) => {
            app.push(Role::Tool, format!("{name} {summary}"));
        }
        UiEvent::Agent(AgentEvent::ToolDone { name, error }) => {
            if let Some(error) = error {
                app.push(Role::Event, format!("{name} failed: {error}"));
            }
        }
        UiEvent::Agent(AgentEvent::Usage(usage)) => {
            app.usage = format!(
                "↑{} ↓{} R{} W{}",
                format_tokens(usage.input),
                format_tokens(usage.output),
                format_tokens(usage.cache_read),
                format_tokens(usage.cache_write)
            );
        }
        UiEvent::Done(result) => {
            if let Err(error) = result {
                app.push(Role::Event, format!("error: {error}"));
            }
            app.busy = false;
        }
        UiEvent::Cancelled => {
            app.push(Role::Event, "cancelled");
            app.busy = false;
        }
        UiEvent::Sessions(result) => {
            match result {
                Ok(sessions) if sessions.is_empty() => {
                    app.push(Role::Event, "no session to resume");
                }
                Ok(sessions) => {
                    app.picker = Some(Picker {
                        sessions,
                        selected: 0,
                    });
                }
                Err(error) => app.push(Role::Event, format!("error: {error}")),
            }
            app.busy = false;
        }
        UiEvent::Resumed(result) => {
            match result {
                Ok(messages) => {
                    app.messages.clear();
                    replay_messages(app, &messages);
                    app.push(Role::Event, "resumed session");
                }
                Err(error) => app.push(Role::Event, format!("error: {error}")),
            }
            app.busy = false;
        }
    }
}

fn replay_messages(app: &mut App, messages: &[Value]) {
    for message in messages {
        let role = message["role"].as_str().unwrap_or_default();
        if let Some(text) = message["content"].as_str() {
            app.push(Role::User, text);
            continue;
        }
        for block in message["content"].as_array().into_iter().flatten() {
            match (role, block["type"].as_str().unwrap_or_default()) {
                ("assistant", "text") => {
                    app.push(Role::Assistant, block["text"].as_str().unwrap_or_default());
                }
                ("assistant", "thinking") => {
                    app.push(
                        Role::Thinking,
                        block["thinking"].as_str().unwrap_or_default(),
                    );
                }
                ("assistant", "tool_use") => {
                    let name = block["name"].as_str().unwrap_or_default();
                    app.push(
                        Role::Tool,
                        format!("{name} {}", tools::summary(name, &block["input"])),
                    );
                }
                ("user", "text") => {
                    app.push(Role::User, block["text"].as_str().unwrap_or_default());
                }
                _ => {}
            }
        }
    }
}

fn format_tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..999_950 => format!("{:.1}k", count as f64 / 1_000.0),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let input_width = frame.area().width.max(1) as usize;
    let input_characters: Vec<char> = app.input.chars().collect();
    let cursor_row = input_characters.len() / input_width;
    let cursor_column = input_characters.len() % input_width;
    let input_lines: Vec<Line> = (0..=cursor_row)
        .map(|row| {
            let start = (row * input_width).min(input_characters.len());
            let end = (start + input_width).min(input_characters.len());
            Line::raw(input_characters[start..end].iter().collect::<String>())
        })
        .collect();
    let [chat, input, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(input_lines.len().min(u16::MAX as usize - 2) as u16 + 2),
            Constraint::Length(1),
        ])
        .areas(frame.area());

    let chat = chat.inner(Margin::new(1, 0));
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
        lines.push(Line::from(app.status.as_str().dark_gray()));
    }

    let conversation = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let viewport_height = chat.height;
    let wrapped_line_count = conversation.line_count(chat.width);
    app.max_scroll = wrapped_line_count
        .saturating_sub(viewport_height as usize)
        .min(u16::MAX as usize) as u16;
    app.page_size = viewport_height.max(1);
    app.scroll_from_bottom = app.scroll_from_bottom.min(app.max_scroll);
    let scroll = app.max_scroll.saturating_sub(app.scroll_from_bottom);
    if let Some(picker) = &app.picker {
        frame.render_widget(picker_view(picker, chat.height), chat);
    } else {
        frame.render_widget(conversation.scroll((scroll, 0)), chat);
    }

    let input_scroll = (cursor_row as u16).saturating_sub(input.height.saturating_sub(3));
    frame.render_widget(
        Paragraph::new(input_lines)
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::BOTTOM)
                    .border_style(thinking_colour(app.thinking_level)),
            )
            .scroll((input_scroll, 0)),
        input,
    );
    frame.set_cursor_position((
        input.x + cursor_column as u16,
        input.y + cursor_row as u16 - input_scroll + 1,
    ));

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!(" {}:{}", app.model, app.thinking_level)),
            Span::raw(if app.usage.is_empty() {
                "".into()
            } else {
                format!(" · {}", app.usage)
            }),
        ])),
        footer,
    );
}

fn picker_view(picker: &Picker, height: u16) -> Paragraph<'_> {
    let mut lines = vec![Line::from(
        "Resume session (↑↓ select, Enter resume, Esc cancel)".bold(),
    )];
    let visible = (height as usize).saturating_sub(1).max(1);
    let first = (picker.selected + 1).saturating_sub(visible);
    for (index, session) in picker.sessions.iter().enumerate().skip(first).take(visible) {
        let preview = session.preview.lines().next().unwrap_or_default();
        let text = format!("{:>8}  {preview}", time_ago(session.modified));
        lines.push(if index == picker.selected {
            Line::from(format!("› {text}").reversed())
        } else {
            Line::raw(format!("  {text}"))
        });
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

fn thinking_colour(level: &str) -> Color {
    match level {
        "low" => Color::Green,
        "medium" => Color::Cyan,
        "high" => Color::Blue,
        "xhigh" => Color::Magenta,
        "max" => Color::Red,
        _ => Color::Reset,
    }
}

fn user_message_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(2).max(1);
    let mut rows = vec![String::new()];
    for line in text.lines() {
        let characters: Vec<char> = line.chars().collect();
        if characters.is_empty() {
            rows.push(String::new());
        }
        for chunk in characters.chunks(content_width) {
            rows.push(chunk.iter().collect());
        }
    }
    rows.push(String::new());
    rows.into_iter()
        .map(|row| {
            let padding = content_width.saturating_sub(row.chars().count()) + 1;
            Line::from(format!(" {row}{}", " ".repeat(padding))).style(
                Style::new()
                    .fg(Color::Rgb(59, 63, 65))
                    .bg(Color::Rgb(223, 231, 236)),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::format_tokens;

    #[test]
    fn formats_tokens() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(12_345), "12.3k");
        assert_eq!(format_tokens(999_950), "1.0M");
        assert_eq!(format_tokens(1_234_567), "1.2M");
    }
}
