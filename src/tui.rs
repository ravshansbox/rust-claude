use crate::{
    agent::{Agent, AgentEvent, Stats, THINKING_LEVELS},
    session::SessionSummary,
    settings::Settings,
    tools,
};
use anyhow::Result;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind,
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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    workspace: String,
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
    prompt_history: Vec<String>,
    history_index: Option<usize>,
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
            workspace: workspace_label(),
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
            prompt_history: Vec::new(),
            history_index: None,
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

    fn previous_prompt(&mut self) {
        let index = match self.history_index {
            Some(index) => index.saturating_sub(1),
            None if self.prompt_history.is_empty() => return,
            None => self.prompt_history.len() - 1,
        };
        self.history_index = Some(index);
        self.input = self.prompt_history[index].clone();
    }

    fn next_prompt(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.prompt_history.len() {
            self.history_index = Some(index + 1);
            self.input = self.prompt_history[index + 1].clone();
        } else {
            self.history_index = None;
            self.input.clear();
        }
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
    if let Err(error) = execute!(std::io::stdout(), EnableMouseCapture, EnableBracketedPaste) {
        ratatui::restore();
        return Err(error.into());
    }

    let result = run_loop(&mut terminal, agent).await;
    let mouse_result = execute!(
        std::io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste
    );
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

fn notify_renewed(agent: &mut Agent, events: &mpsc::UnboundedSender<UiEvent>) {
    if agent.take_renewed() {
        let _ = events.send(UiEvent::Agent(AgentEvent::Notice(
            "renewed sign-in token".into(),
        )));
    }
}

async fn agent_task(
    mut agent: Agent,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut cancel: mpsc::UnboundedReceiver<()>,
    events: mpsc::UnboundedSender<UiEvent>,
) {
    if agent.load_quota().await.is_ok() {
        let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
    }
    notify_renewed(&mut agent, &events);
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
                notify_renewed(&mut agent, &events);
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
                if app.model != saved_model || app.thinking_level != saved_thinking_level {
                    saved_model = app.model.clone();
                    saved_thinking_level = app.thinking_level;
                    let settings = Settings {
                        model: Some(saved_model.clone()),
                        thinking_level: Some(saved_thinking_level.to_string()),
                    };
                    if let Err(error) = settings.save() {
                        app.push(Role::Event, format!("failed to save settings: {error}"));
                    }
                }
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
    if let Event::Paste(text) = event {
        if app.picker.is_none() {
            app.input
                .push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
            app.history_index = None;
            app.command_selected = 0;
            app.commands_dismissed = false;
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
        (KeyCode::Char('c'), KeyModifiers::CONTROL) if !app.input.is_empty() => {
            app.input.clear();
            app.history_index = None;
            app.command_selected = 0;
            app.commands_dismissed = false;
        }
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return true,
        (KeyCode::Esc, _) if app.busy => {
            if app.status == "working" {
                app.status = "cancelling".into();
                act(Action::Cancel);
            }
        }
        (KeyCode::Esc, _) => return true,
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => return true,
        (KeyCode::BackTab, _) => app.cycle_thinking_level(),
        (KeyCode::Up, _) if app.input.is_empty() || app.history_index.is_some() => {
            app.previous_prompt();
        }
        (KeyCode::Down, _) if app.history_index.is_some() => app.next_prompt(),
        (KeyCode::Up, _) => app.scroll_up(1),
        (KeyCode::Down, _) => app.scroll_down(1),
        (KeyCode::PageUp, _) => app.scroll_up(app.page_size),
        (KeyCode::PageDown, _) => app.scroll_down(app.page_size),
        (KeyCode::Home, _) => app.scroll_to_top(),
        (KeyCode::End, _) => app.scroll_to_bottom(),
        (KeyCode::Enter, _) if !app.busy && !app.input.trim().is_empty() => {
            app.scroll_to_bottom();
            let prompt = std::mem::take(&mut app.input);
            app.history_index = None;
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
            app.prompt_history.push(prompt.clone());
            app.status = "working".into();
            app.busy = true;
            act(Action::Submit(prompt, app.thinking_level));
        }
        (KeyCode::Backspace, _) => {
            app.input.pop();
            app.history_index = None;
            app.command_selected = 0;
            app.commands_dismissed = false;
        }
        (KeyCode::Char(character), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.push(character);
            app.history_index = None;
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
                    app.prompt_history.clear();
                    app.history_index = None;
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
                    app.prompt_history.clear();
                    app.history_index = None;
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
            app.prompt_history.push(text.to_string());
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
                    let text = block["text"].as_str().unwrap_or_default();
                    app.push(Role::User, text);
                    app.prompt_history.push(text.to_string());
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

fn format_quota(stats: &Stats) -> String {
    [
        (
            "5h",
            stats.quota.five_hour_remaining,
            stats.quota.five_hour_reset,
        ),
        (
            "7d",
            stats.quota.seven_day_remaining,
            stats.quota.seven_day_reset,
        ),
    ]
    .into_iter()
    .filter_map(|(label, remaining, reset)| {
        remaining.map(|remaining| {
            let reset = reset
                .map(|reset| format!(" {}", time_until(reset)))
                .unwrap_or_default();
            format!("{label} {remaining:.0}%{reset}")
        })
    })
    .collect::<Vec<_>>()
    .join(" · ")
}

fn time_until(reset: u64) -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_duration(reset.saturating_sub(now))
}

fn format_duration(seconds: u64) -> String {
    let minutes = seconds / 60;
    let (days, hours, minutes) = (minutes / 1_440, minutes / 60 % 24, minutes % 60);
    match (days, hours, minutes) {
        (0, 0, 0) => "<1m".to_string(),
        (0, 0, minutes) => format!("{minutes}m"),
        (0, hours, 0) => format!("{hours}h"),
        (0, hours, minutes) => format!("{hours}h{minutes}m"),
        (days, 0, _) => format!("{days}d"),
        (days, hours, _) => format!("{days}d{hours}h"),
    }
}

fn context_span(stats: &Stats) -> Span<'static> {
    let percent = if stats.context_window > 0 {
        stats.context_tokens as f64 / stats.context_window as f64 * 100.0
    } else {
        0.0
    };
    Span::raw(format!(
        "{percent:.1}%/{}",
        format_tokens(stats.context_window)
    ))
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
    let input_rows = input_rows(&app.input, frame.area().width.max(1) as usize);
    let cursor_row = input_rows.len() - 1;
    let cursor_column = input_rows[cursor_row].width();
    let input_lines: Vec<Line> = input_rows.into_iter().map(Line::raw).collect();
    let mut footer_lines = vec![
        Line::raw(format!(
            "{} · {}:{}",
            app.workspace,
            display_model(&app.model),
            app.thinking_level
        )),
        Line::from(vec![
            context_span(&app.stats),
            Span::raw(match format_stats(&app.stats) {
                stats if stats.is_empty() => stats,
                stats => format!(" {stats}"),
            }),
        ]),
    ];
    let quota = format_quota(&app.stats);
    if !quota.is_empty() {
        footer_lines.push(Line::raw(quota));
    }
    let footer_paragraph = Paragraph::new(footer_lines).wrap(Wrap { trim: false });
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

    frame.render_widget(footer_paragraph, footer);
}

fn input_rows(input: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut row_width = 0;
    for character in input.chars() {
        if character == '\n' {
            rows.push(String::new());
            row_width = 0;
            continue;
        }
        let character_width = character.width().unwrap_or(0);
        if row_width > 0 && row_width + character_width > width {
            rows.push(String::new());
            row_width = 0;
        }
        if let Some(row) = rows.last_mut() {
            row.push(character);
        }
        row_width += character_width;
    }
    if row_width >= width {
        rows.push(String::new());
    }
    rows
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
        let mut line_rows = input_rows(line, content_width);
        if line_rows.len() > 1 && line_rows.last().is_some_and(String::is_empty) {
            line_rows.pop();
        }
        rows.extend(line_rows);
    }
    rows.push(String::new());
    rows.into_iter()
        .map(|row| {
            let padding = content_width.saturating_sub(row.width()) + 1;
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
    use super::{
        Action, App, format_duration, format_tokens, handle_input, input_rows, user_message_lines,
    };
    use crate::agent::{Quota, Stats, Usage};
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn ignores_escape_while_loading_models() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        app.busy = true;
        app.status = "loading models".into();
        let mut cancelled = false;
        let quit = handle_input(
            Event::Key(KeyEvent::from(KeyCode::Esc)),
            &mut app,
            |action| {
                cancelled |= matches!(action, Action::Cancel);
            },
        );
        assert!(!quit);
        assert!(!cancelled);
        assert_eq!(app.status, "loading models");
    }

    #[test]
    fn ignores_unbound_control_keys() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        app.input = "hello".into();
        for character in ['a', 'd'] {
            let quit = handle_input(
                Event::Key(KeyEvent::new(
                    KeyCode::Char(character),
                    KeyModifiers::CONTROL,
                )),
                &mut app,
                |_| {},
            );
            assert!(!quit);
        }
        assert_eq!(app.input, "hello");
    }

    #[test]
    fn wraps_user_message_by_display_width() {
        let rows: Vec<String> = user_message_lines("日本語のテキスト\n\nabcd", 8)
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(
            rows,
            vec![
                "        ",
                " 日本語 ",
                " のテキ ",
                " スト   ",
                "        ",
                " abcd   ",
                "        ",
            ]
        );
    }

    #[test]
    fn wraps_input_by_display_width() {
        assert_eq!(input_rows("你好世界", 5), vec!["你好", "世界"]);
        assert_eq!(input_rows("abcd", 4), vec!["abcd", ""]);
        assert_eq!(input_rows("", 4), vec![""]);
        assert_eq!(input_rows("ab\ncd\n", 4), vec!["ab", "cd", ""]);
    }

    #[test]
    fn pastes_multiple_lines_without_submitting() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        let mut submitted = false;
        let quit = handle_input(Event::Paste("first\r\nsecond".into()), &mut app, |action| {
            submitted |= matches!(action, Action::Submit(..));
        });
        assert!(!quit);
        assert!(!submitted);
        assert_eq!(app.input, "first\nsecond");
    }

    #[test]
    fn walks_through_prompt_history() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        for prompt in ["first", "second"] {
            handle_input(Event::Paste(prompt.into()), &mut app, |_| {});
            handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {});
            app.busy = false;
        }
        let press = |app: &mut App, code| {
            handle_input(Event::Key(KeyEvent::from(code)), app, |_| {});
            app.input.clone()
        };
        assert_eq!(press(&mut app, KeyCode::Up), "second");
        assert_eq!(press(&mut app, KeyCode::Up), "first");
        assert_eq!(press(&mut app, KeyCode::Up), "first");
        assert_eq!(press(&mut app, KeyCode::Down), "second");
        assert_eq!(press(&mut app, KeyCode::Down), "");
        app.input = "draft".into();
        assert_eq!(press(&mut app, KeyCode::Up), "draft");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0), "<1m");
        assert_eq!(format_duration(59), "<1m");
        assert_eq!(format_duration(45 * 60), "45m");
        assert_eq!(format_duration(2 * 3_600), "2h");
        assert_eq!(format_duration(2 * 3_600 + 13 * 60), "2h13m");
        assert_eq!(format_duration(3 * 86_400), "3d");
        assert_eq!(format_duration(3 * 86_400 + 4 * 3_600 + 5 * 60), "3d4h");
    }

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
        Some(branch) => format!("{folder} ({branch})"),
        None => folder,
    }
}

fn display_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}
