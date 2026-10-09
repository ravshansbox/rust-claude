use crate::{
    agent::{Agent, AgentEvent, Stats, THINKING_LEVELS},
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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use serde_json::Value;
use std::time::{Duration, SystemTime};
use tokio::{sync::mpsc, time::MissedTickBehavior};

const REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const COMMANDS: &[(&str, &str)] = &[
    ("/new", "start a new session"),
    ("/resume", "resume a previous session"),
    ("/model", "select model"),
    ("/thinking", "select thinking level"),
    ("/quit", "quit"),
];

fn command_matches(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|(name, _)| name.starts_with(input))
        .copied()
        .collect()
}

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
        Role::Tool => tool_message_lines(text),
        Role::Event => text
            .lines()
            .map(|line| Line::raw(line.to_string()))
            .collect(),
    }
}

fn tool_message_lines(text: &str) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = text
        .lines()
        .map(|line| Line::raw(line.to_string()))
        .collect();
    if let Some(first) = text.lines().next() {
        let (name, rest) = first.split_once(' ').unwrap_or((first, ""));
        lines[0] = Line::from(vec![
            Span::styled(
                format!(" {name} "),
                Style::new()
                    .fg(Color::Rgb(59, 63, 65))
                    .bg(Color::Rgb(223, 231, 236)),
            ),
            Span::raw(format!(" {rest}")),
        ]);
    }
    lines
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
    spinner_frame: usize,
    stats: Stats,
    scroll_from_bottom: u16,
    max_scroll: u16,
    page_size: u16,
    busy: bool,
    picker: Option<Picker>,
    command_selected: usize,
    commands_dismissed: bool,
}

#[derive(Clone, Copy)]
enum PickerKind {
    Session,
    Model,
    Thinking,
}

struct Picker {
    kind: PickerKind,
    title: &'static str,
    items: Vec<(String, String)>,
    selected: usize,
}

impl App {
    fn new(model: &str, thinking_level: &'static str, stats: Stats) -> Self {
        let mut app = Self {
            input: String::new(),
            messages: Vec::new(),
            model: model.into(),
            thinking_level,
            status: String::new(),
            spinner_frame: 0,
            stats,
            scroll_from_bottom: 0,
            max_scroll: 0,
            page_size: 1,
            busy: false,
            picker: None,
            command_selected: 0,
            commands_dismissed: false,
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

    fn visible_commands(&self) -> Vec<(&'static str, &'static str)> {
        if self.commands_dismissed {
            return Vec::new();
        }
        command_matches(&self.input)
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
    Cancelled(Result<()>),
    Sessions(Result<Vec<SessionSummary>>),
    Resumed(Result<Vec<Value>>),
    NewSession(Result<()>),
    Models(Result<Vec<String>>),
}

enum Request {
    Prompt(String, &'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    NewSession,
    ListModels,
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
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
            Request::SetModel(model) => {
                agent.model = model;
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
            Request::NewSession => {
                let _ = events.send(UiEvent::NewSession(agent.new_session()));
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
            Request::ListModels => {
                let _ = events.send(UiEvent::Models(agent.list_models().await));
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
            let _ = events.send(UiEvent::Cancelled(agent.cancel(checkpoint)));
        }
        let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
    }
}

async fn run_loop(terminal: &mut DefaultTerminal, agent: Agent) -> Result<()> {
    let mut app = App::new(&agent.model, agent.thinking_level, agent.stats());
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
    let mut spinner = tokio::time::interval(SPINNER_INTERVAL);
    spinner.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;

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
    NewSession,
    ListModels,
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
            KeyCode::Down => picker.selected = (picker.selected + 1).min(picker.items.len() - 1),
            KeyCode::Enter => {
                let kind = picker.kind;
                let value = picker.items[picker.selected].0.clone();
                app.picker = None;
                match kind {
                    PickerKind::Session => {
                        app.status = "resuming".into();
                        app.busy = true;
                        act(Action::Resume(value));
                    }
                    PickerKind::Thinking => {
                        if let Some(level) = THINKING_LEVELS.iter().find(|level| **level == value) {
                            app.thinking_level = level;
                            app.push(Role::Event, format!("thinking: {level}"));
                        }
                    }
                    PickerKind::Model => {
                        app.model = value.clone();
                        app.push(Role::Event, format!("model: {}", display_model(&value)));
                        act(Action::SetModel(value));
                    }
                }
            }
            KeyCode::Esc => app.picker = None,
            KeyCode::Char('c') if key.modifiers == KeyModifiers::CONTROL => return true,
            _ => {}
        }
        return false;
    }

    let matches = app.visible_commands();
    if !matches.is_empty() {
        app.command_selected = app.command_selected.min(matches.len() - 1);
        match key.code {
            KeyCode::Up => {
                app.command_selected = app.command_selected.saturating_sub(1);
                return false;
            }
            KeyCode::Down => {
                app.command_selected = (app.command_selected + 1).min(matches.len() - 1);
                return false;
            }
            KeyCode::Enter if !app.busy => {
                app.input = matches[app.command_selected].0.to_string();
            }
            KeyCode::Tab => {
                app.input = matches[app.command_selected].0.to_string();
                app.command_selected = 0;
                return false;
            }
            KeyCode::Esc => {
                app.commands_dismissed = true;
                return false;
            }
            _ => {}
        }
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
                    "/new" => {
                        app.status = "starting new session".into();
                        app.busy = true;
                        act(Action::NewSession);
                    }
                    "/thinking" if argument.is_empty() => {
                        app.picker = Some(Picker {
                            kind: PickerKind::Thinking,
                            title: "Select thinking level",
                            items: THINKING_LEVELS
                                .iter()
                                .map(|level| (level.to_string(), level.to_string()))
                                .collect(),
                            selected: THINKING_LEVELS
                                .iter()
                                .position(|level| *level == app.thinking_level)
                                .unwrap_or_default(),
                        });
                    }
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
                        app.status = "loading models".into();
                        app.busy = true;
                        act(Action::ListModels);
                    }
                    "/model" => {
                        app.model = argument.to_string();
                        app.push(Role::Event, format!("model: {}", display_model(argument)));
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
            app.status = "working".into();
            app.busy = true;
            act(Action::Submit(prompt, app.thinking_level));
        }
        (KeyCode::Backspace, _) => {
            app.input.pop();
            app.command_selected = 0;
            app.commands_dismissed = false;
        }
        (KeyCode::Char(character), _) => {
            app.input.push(character);
            app.command_selected = 0;
            app.commands_dismissed = false;
        }
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
        UiEvent::Agent(AgentEvent::Notice(text)) => app.push(Role::Event, text),
        UiEvent::Agent(AgentEvent::Stats(stats)) => app.stats = stats,
        UiEvent::Done(result) => {
            if let Err(error) = result {
                app.push(Role::Event, format!("error: {error}"));
            }
            app.busy = false;
        }
        UiEvent::Cancelled(result) => {
            app.push(Role::Event, "cancelled");
            if let Err(error) = result {
                app.push(Role::Event, format!("error: {error}"));
            }
            app.busy = false;
        }
        UiEvent::Sessions(result) => {
            match result {
                Ok(sessions) if sessions.is_empty() => {
                    app.push(Role::Event, "no session to resume");
                }
                Ok(sessions) => {
                    app.picker = Some(Picker {
                        kind: PickerKind::Session,
                        title: "Resume session",
                        items: sessions
                            .into_iter()
                            .map(|session| {
                                let preview = session.preview.lines().next().unwrap_or_default();
                                let label = format!("{:>8}  {preview}", time_ago(session.modified));
                                (session.id, label)
                            })
                            .collect(),
                        selected: 0,
                    });
                }
                Err(error) => app.push(Role::Event, format!("error: {error}")),
            }
            app.busy = false;
        }
        UiEvent::NewSession(result) => {
            match result {
                Ok(()) => {
                    app.messages.clear();
                    app.push(Role::Event, "new session");
                }
                Err(error) => app.push(Role::Event, format!("error: {error}")),
            }
            app.busy = false;
        }
        UiEvent::Models(result) => {
            match result {
                Ok(models) if models.is_empty() => app.push(Role::Event, "no models available"),
                Ok(models) => {
                    let selected = models
                        .iter()
                        .position(|model| *model == app.model)
                        .unwrap_or_default();
                    app.picker = Some(Picker {
                        kind: PickerKind::Model,
                        title: "Select model",
                        items: models
                            .into_iter()
                            .map(|model| (model.clone(), display_model(&model).to_string()))
                            .collect(),
                        selected,
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

fn format_stats(stats: &Stats) -> String {
    let usage = &stats.usage;
    let mut parts: Vec<String> = [
        ("↑", usage.input),
        ("↓", usage.output),
        ("R", usage.cache_read),
        ("W", usage.cache_write),
    ]
    .into_iter()
    .filter(|(_, count)| *count > 0)
    .map(|(label, count)| format!("{label}{}", format_tokens(count)))
    .collect();
    if let Some(rate) = stats.cache_hit_rate
        && (usage.cache_read > 0 || usage.cache_write > 0)
    {
        parts.push(format!("CH{rate:.1}%"));
    }
    parts.join(" ")
}

fn context_span(stats: &Stats) -> Span<'static> {
    let percent = if stats.context_window > 0 {
        stats.context_tokens as f64 / stats.context_window as f64 * 100.0
    } else {
        0.0
    };
    let text = format!("{percent:.1}%/{}", format_tokens(stats.context_window));
    if percent > 90.0 {
        Span::styled(text, Style::default().fg(Color::Red))
    } else if percent > 70.0 {
        Span::styled(text, Style::default().fg(Color::Yellow))
    } else {
        Span::raw(text)
    }
}

fn format_tokens(count: u64) -> String {
    let count = count as f64;
    if count < 1_000.0 {
        count.to_string()
    } else if count < 10_000.0 {
        format!("{:.1}k", count / 1_000.0)
    } else if count < 1_000_000.0 {
        format!("{}k", (count / 1_000.0).round())
    } else if count < 10_000_000.0 {
        format!("{:.1}M", count / 1_000_000.0)
    } else {
        format!("{}M", (count / 1_000_000.0).round())
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

    let matches = app.visible_commands();
    if !matches.is_empty() {
        let height = (matches.len() as u16).min(chat.height);
        let area = Rect {
            y: chat.y + chat.height - height,
            height,
            ..chat
        };
        let selected = app.command_selected.min(matches.len() - 1);
        let lines: Vec<Line> = matches
            .iter()
            .enumerate()
            .map(|(index, (name, description))| {
                let text = format!("{name:<12}{description}");
                if index == selected {
                    Line::from(text.reversed())
                } else {
                    Line::raw(text)
                }
            })
            .collect();
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines), area);
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
            Span::raw(format!("{}:{}", display_model(&app.model), app.thinking_level)),
            Span::raw(format!(" · {} ", format_stats(&app.stats))),
            context_span(&app.stats),
        ])),
        footer,
    );
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
        assert_eq!(format_tokens(1_234), "1.2k");
        assert_eq!(format_tokens(12_345), "12k");
        assert_eq!(format_tokens(999_999), "1000k");
        assert_eq!(format_tokens(1_234_567), "1.2M");
        assert_eq!(format_tokens(12_345_678), "12M");
    }
}

fn display_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}
