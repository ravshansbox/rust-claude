mod input;
mod render;
mod status;

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
use input::{input_cursor, input_rows, next_word_end, previous_word_start};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use render::{THEME, Theme, borrowed_line, render_message, tool_message};
use serde_json::Value;
use status::{context_span, format_quota, format_stats};
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

struct App {
    input: String,
    cursor: usize,
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
            cursor: 0,
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
        self.cursor = self.input.len();
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
        self.cursor = self.input.len();
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

    fn input_changed(&mut self) {
        self.history_index = None;
        self.command_selected = 0;
        self.commands_dismissed = false;
    }

    fn start(&mut self, status: &str) {
        self.status = status.into();
        self.busy = true;
    }

    fn set_thinking_level(&mut self, name: &str) {
        match THINKING_LEVELS.iter().find(|level| **level == name) {
            Some(level) => {
                self.thinking_level = level;
                self.push(Role::Event, format!("thinking: {level}"));
            }
            None => self.push(
                Role::Event,
                format!(
                    "unknown thinking level: {name} (options: {})",
                    THINKING_LEVELS.join(", ")
                ),
            ),
        }
    }

    fn set_model(&mut self, model: &str) {
        self.model = model.into();
        self.push(Role::Event, format!("model: {}", display_model(model)));
    }

    fn clear_session(&mut self) {
        self.messages.clear();
        self.prompt_history.clear();
        self.history_index = None;
    }

    fn finish<T>(&mut self, result: Result<T>, on_success: impl FnOnce(&mut Self, T)) {
        match result {
            Ok(value) => on_success(self, value),
            Err(error) => self.push(Role::Event, format!("error: {error}")),
        }
        self.busy = false;
    }
}

pub async fn run(agent: Agent) -> Result<()> {
    THEME.get_or_init(Theme::detect);
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
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            app.input.insert_str(app.cursor, &text);
            app.cursor += text.len();
            app.input_changed();
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
                        app.start("resuming");
                        act(Action::Resume(value));
                    }
                    PickerKind::Thinking => app.set_thinking_level(&value),
                    PickerKind::Model => {
                        app.set_model(&value);
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
                app.cursor = app.input.len();
            }
            KeyCode::Tab => {
                app.input = matches[app.command_selected].0.to_string();
                app.cursor = app.input.len();
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
            app.cursor = 0;
            app.input_changed();
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
            app.cursor = 0;
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
                        app.start("starting new session");
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
                    "/thinking" => app.set_thinking_level(argument),
                    "/model" if argument.is_empty() => {
                        app.start("loading models");
                        act(Action::ListModels);
                    }
                    "/model" => {
                        app.set_model(argument);
                        act(Action::SetModel(argument.to_string()));
                    }
                    "/resume" => {
                        app.start("loading sessions");
                        act(Action::ListSessions);
                    }
                    command => app.push(Role::Event, format!("unknown command: {command}")),
                }
                return false;
            }
            app.push(Role::User, prompt.clone());
            app.prompt_history.push(prompt.clone());
            app.start("working");
            act(Action::Submit(prompt, app.thinking_level));
        }
        (KeyCode::Left | KeyCode::Char('b'), KeyModifiers::ALT) => {
            app.cursor = previous_word_start(&app.input, app.cursor);
        }
        (KeyCode::Right | KeyCode::Char('f'), KeyModifiers::ALT) => {
            app.cursor = next_word_end(&app.input, app.cursor);
        }
        (KeyCode::Char('a'), KeyModifiers::CONTROL) => app.cursor = 0,
        (KeyCode::Char('e'), KeyModifiers::CONTROL) => app.cursor = app.input.len(),
        (KeyCode::Left, _) => {
            if let Some(character) = app.input[..app.cursor].chars().next_back() {
                app.cursor -= character.len_utf8();
            }
        }
        (KeyCode::Right, _) => {
            if let Some(character) = app.input[app.cursor..].chars().next() {
                app.cursor += character.len_utf8();
            }
        }
        (KeyCode::Backspace, KeyModifiers::ALT) | (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
            let start = previous_word_start(&app.input, app.cursor);
            app.input.replace_range(start..app.cursor, "");
            app.cursor = start;
            app.input_changed();
        }
        (KeyCode::Backspace, _) => {
            if let Some(character) = app.input[..app.cursor].chars().next_back() {
                app.cursor -= character.len_utf8();
                app.input.remove(app.cursor);
            }
            app.input_changed();
        }
        (KeyCode::Char(character), modifiers) if !modifiers.contains(KeyModifiers::CONTROL) => {
            app.input.insert(app.cursor, character);
            app.cursor += character.len_utf8();
            app.input_changed();
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
        UiEvent::Agent(AgentEvent::ToolStart {
            name,
            summary,
            diff,
        }) => {
            app.push(Role::Tool, tool_message(&name, summary, diff));
        }
        UiEvent::Agent(AgentEvent::ToolDone { name, error }) => {
            if let Some(error) = error {
                app.push(Role::Event, format!("{name} failed: {error}"));
            }
        }
        UiEvent::Agent(AgentEvent::Notice(text)) => app.push(Role::Event, text),
        UiEvent::Agent(AgentEvent::Stats(stats)) => app.stats = stats,
        UiEvent::Done(result) => app.finish(result, |_, ()| {}),
        UiEvent::Cancelled(result) => {
            app.push(Role::Event, "cancelled");
            app.finish(result, |_, ()| {});
        }
        UiEvent::Sessions(result) => app.finish(result, |app, sessions| {
            if sessions.is_empty() {
                app.push(Role::Event, "no session to resume");
                return;
            }
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
        }),
        UiEvent::NewSession(result) => app.finish(result, |app, ()| {
            app.clear_session();
            app.push(Role::Event, "new session");
        }),
        UiEvent::Models(result) => app.finish(result, |app, models| {
            if models.is_empty() {
                app.push(Role::Event, "no models available");
                return;
            }
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
        }),
        UiEvent::Resumed(result) => app.finish(result, |app, messages| {
            app.clear_session();
            replay_messages(app, &messages);
            app.push(Role::Event, "resumed session");
        }),
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
                        tool_message(
                            name,
                            tools::summary(name, &block["input"]),
                            tools::diff(name, &block["input"]),
                        ),
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

fn draw(frame: &mut Frame, app: &mut App) {
    let input_width = frame.area().width.max(1) as usize;
    let input_rows = input_rows(&app.input, input_width);
    let (cursor_row, cursor_column) = input_cursor(&app.input, app.cursor, input_width);
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

#[cfg(test)]
mod tests {
    use super::{Action, App, handle_input};
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
        for character in ['g', 'd'] {
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
    fn moves_by_word_with_alt() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        handle_input(Event::Paste("one two".into()), &mut app, |_| {});
        let alt = |code| Event::Key(KeyEvent::new(code, KeyModifiers::ALT));
        handle_input(alt(KeyCode::Left), &mut app, |_| {});
        assert_eq!(app.cursor, "one ".len());
        handle_input(alt(KeyCode::Char('b')), &mut app, |_| {});
        assert_eq!(app.cursor, 0);
        handle_input(alt(KeyCode::Right), &mut app, |_| {});
        assert_eq!(app.cursor, "one".len());
        handle_input(alt(KeyCode::Char('f')), &mut app, |_| {});
        assert_eq!(app.cursor, "one two".len());
        assert_eq!(app.input, "one two");
    }

    #[test]
    fn deletes_previous_word() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        handle_input(Event::Paste("one two three".into()), &mut app, |_| {});
        handle_input(Event::Key(KeyEvent::from(KeyCode::Left)), &mut app, |_| {});
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT)),
            &mut app,
            |_| {},
        );
        assert_eq!(app.input, "one two e");
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)),
            &mut app,
            |_| {},
        );
        assert_eq!(app.input, "one e");
        assert_eq!(app.cursor, "one ".len());
    }

    #[test]
    fn edits_input_at_cursor() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        handle_input(Event::Paste("héllo".into()), &mut app, |_| {});
        for code in [KeyCode::Left, KeyCode::Left, KeyCode::Left, KeyCode::Left] {
            handle_input(Event::Key(KeyEvent::from(code)), &mut app, |_| {});
        }
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Backspace)),
            &mut app,
            |_| {},
        );
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Char('j'))),
            &mut app,
            |_| {},
        );
        handle_input(Event::Key(KeyEvent::from(KeyCode::Right)), &mut app, |_| {});
        handle_input(Event::Paste("!".into()), &mut app, |_| {});
        assert_eq!(app.input, "jé!llo");
        assert_eq!(app.cursor, "jé!".len());
        let control = |character| {
            Event::Key(KeyEvent::new(
                KeyCode::Char(character),
                KeyModifiers::CONTROL,
            ))
        };
        handle_input(control('a'), &mut app, |_| {});
        assert_eq!(app.cursor, 0);
        handle_input(control('e'), &mut app, |_| {});
        assert_eq!(app.cursor, app.input.len());
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
}
