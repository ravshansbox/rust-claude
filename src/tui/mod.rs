mod files;
mod input;
mod render;
mod status;

use crate::{
    agent::{Agent, AgentEvent, Stats, THINKING_LEVELS},
    clipboard,
    images::{self, Image},
    session::SessionSummary,
    settings::Settings,
    skills::{self, Skill},
    tools,
};
use anyhow::Result;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};
use files::{file_matches, file_query, list_files};
use futures::StreamExt;
use input::{input_cursor, input_rows, next_word_end, previous_word_start, row_above, row_below};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    text::{Line, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use render::{THEME, Theme, borrowed_line, render_message, theme, tool_message};
use serde_json::Value;
use status::{format_quota, format_stats};
use std::time::{Duration, SystemTime};
use tokio::{sync::mpsc, time::MissedTickBehavior};

pub fn dark_theme() -> bool {
    matches!(Theme::detect(), Theme::Dark)
}

const REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

const COMMANDS: &[(&str, &str)] = &[
    ("/new", "start a new session"),
    ("/compact", "summarise the conversation to free context"),
    ("/resume", "resume a previous session"),
    ("/model", "select model"),
    ("/thinking", "select thinking level"),
    ("/quit", "quit"),
];

fn command_matches(input: &str, skills: &[Skill]) -> Vec<(String, String)> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let commands = COMMANDS
        .iter()
        .map(|(name, description)| (name.to_string(), description.to_string()));
    let skill_commands = skills.iter().map(|skill| {
        (
            format!("/skill:{}", skill.name),
            skill
                .description
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        )
    });
    commands
        .chain(skill_commands)
        .filter(|(name, _)| name.starts_with(input))
        .collect()
}

struct Suggestions {
    start: usize,
    end: usize,
    items: Vec<(String, String)>,
    files: bool,
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
    input_width: usize,
    busy: bool,
    picker: Option<Picker>,
    command_selected: usize,
    commands_dismissed: bool,
    files: Option<Vec<String>>,
    prompt_history: Vec<String>,
    history_index: Option<usize>,
    reads: tools::ReadGroup,
    skills: Vec<Skill>,
    images: Vec<(usize, Image)>,
    image_count: usize,
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
            input_width: usize::MAX,
            busy: false,
            picker: None,
            command_selected: 0,
            commands_dismissed: false,
            files: None,
            prompt_history: Vec::new(),
            history_index: None,
            reads: tools::ReadGroup::default(),
            skills: Vec::new(),
            images: Vec::new(),
            image_count: 0,
        };
        app.push(
            Role::Event,
            "Ask me to inspect, explain, or edit this project.",
        );
        app
    }

    fn push(&mut self, role: Role, text: impl Into<String>) {
        self.reads.clear();
        self.messages.push(ChatMessage {
            role,
            text: text.into(),
            rendered: None,
        });
    }

    fn push_tool(&mut self, name: &str, summary: String, diff: Option<String>) {
        if name != "read" || diff.is_some() {
            self.push(Role::Tool, tool_message(name, summary, diff));
            return;
        }
        let merge = !self.reads.is_empty();
        let mut reads = std::mem::take(&mut self.reads);
        reads.add(summary);
        let text = tool_message(name, reads.summary(), None);
        match self.messages.last_mut() {
            Some(last) if merge => {
                last.text = text;
                last.rendered = None;
            }
            _ => self.push(Role::Tool, text),
        }
        self.reads = reads;
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

    fn visible_suggestions(&self) -> Option<Suggestions> {
        if self.commands_dismissed {
            return None;
        }
        let commands = command_matches(&self.input, &self.skills);
        if !commands.is_empty() {
            return Some(Suggestions {
                start: 0,
                end: self.input.len(),
                items: commands,
                files: false,
            });
        }
        let (start, query) = file_query(&self.input, self.cursor)?;
        let items: Vec<(String, String)> = file_matches(self.files.as_deref()?, query)
            .into_iter()
            .map(|path| (path, String::new()))
            .collect();
        if items.is_empty() {
            return None;
        }
        Some(Suggestions {
            start,
            end: self.cursor,
            items,
            files: true,
        })
    }

    fn accept_suggestion(&mut self, suggestions: &Suggestions) {
        let name = &suggestions.items[self.command_selected].0;
        let replacement = if suggestions.files {
            format!("@{name} ")
        } else {
            name.clone()
        };
        self.input
            .replace_range(suggestions.start..suggestions.end, &replacement);
        self.cursor = suggestions.start + replacement.len();
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
        if self.files.is_none() && file_query(&self.input, self.cursor).is_some() {
            self.files = Some(list_files());
        }
    }

    fn attach_image(&mut self, image: Image) {
        self.image_count += 1;
        let marker = image_marker(self.image_count);
        self.input.insert_str(self.cursor, &marker);
        self.cursor += marker.len();
        self.images.push((self.image_count, image));
        self.input_changed();
    }

    fn take_images(&mut self, prompt: &str) -> Vec<Image> {
        std::mem::take(&mut self.images)
            .into_iter()
            .filter(|(number, _)| prompt.contains(&image_marker(*number)))
            .map(|(_, image)| image)
            .collect()
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

fn image_marker(number: usize) -> String {
    format!("[image {number}]")
}

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

enum UiEvent {
    Agent(AgentEvent),
    Done(Result<()>),
    Cancelled(Result<()>),
    Sessions(Result<Vec<SessionSummary>>),
    Resumed(Result<Vec<Value>>),
    NewSession(Result<()>),
    Models(Result<Vec<String>>),
    ModelChecked(Result<String>),
    ImagePasted(Result<Option<Image>>),
}

enum Request {
    Prompt(String, Vec<Image>, &'static str),
    Compact(&'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    CheckModel(String),
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
    let mut quota = agent.quota_request().await.ok().map(tokio::spawn);
    notify_renewed(&mut agent, &events);
    loop {
        let request = tokio::select! {
            Some(result) = async { Some(quota.as_mut()?.await) }, if quota.is_some() => {
                quota = None;
                if let Ok(Ok(value)) = result {
                    agent.merge_quota(value);
                    let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                }
                continue;
            }
            request = requests.recv() => request,
        };
        let Some(request) = request else { break };
        let (prompt, thinking_level) = match request {
            Request::Prompt(prompt, images, thinking_level) => {
                (Some((prompt, images)), thinking_level)
            }
            Request::Compact(thinking_level) => (None, thinking_level),
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
            Request::CheckModel(model) => {
                let result = match agent.list_models().await {
                    Ok(models) if models.contains(&model) => {
                        agent.model = model.clone();
                        Ok(model)
                    }
                    Ok(_) => Err(anyhow::anyhow!("unknown model: {model}")),
                    Err(error) => Err(error),
                };
                let _ = events.send(UiEvent::ModelChecked(result));
                notify_renewed(&mut agent, &events);
                let _ = events.send(UiEvent::Agent(AgentEvent::Stats(agent.stats())));
                continue;
            }
        };
        agent.thinking_level = thinking_level;
        while cancel.try_recv().is_ok() {}

        let checkpoint = agent.history_len();
        let cancelled = {
            let on_event = |event| {
                let _ = events.send(UiEvent::Agent(event));
            };
            let run = async {
                match &prompt {
                    Some((prompt, images)) => agent.prompt(prompt, images, on_event).await,
                    None => agent.compact(on_event).await,
                }
            };
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
    if !agent.skills.skills.is_empty() {
        let names: Vec<&str> = agent
            .skills
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        app.push(Role::Event, format!("loaded skills: {}", names.join(", ")));
    }
    for diagnostic in &agent.skills.diagnostics {
        app.push(Role::Event, diagnostic.to_string());
    }
    app.skills = agent.skills.skills.clone();
    if let Some(loaded) = agent.mcp.loaded() {
        app.push(Role::Event, loaded);
    }
    for diagnostic in &agent.mcp.diagnostics {
        app.push(Role::Event, diagnostic.to_string());
    }
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let image_events = event_tx.clone();
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

enum Action {
    Submit(String, Vec<Image>, &'static str),
    PasteImage,
    Compact(&'static str),
    ListSessions,
    Resume(String),
    SetModel(String),
    CheckModel(String),
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

    if key.code == KeyCode::Enter
        && key
            .modifiers
            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT)
    {
        app.input.insert(app.cursor, '\n');
        app.cursor += 1;
        app.input_changed();
        return false;
    }

    if let Some(suggestions) = app.visible_suggestions() {
        let count = suggestions.items.len();
        app.command_selected = app.command_selected.min(count - 1);
        match key.code {
            KeyCode::Up => {
                app.command_selected = app.command_selected.saturating_sub(1);
                return false;
            }
            KeyCode::Down => {
                app.command_selected = (app.command_selected + 1).min(count - 1);
                return false;
            }
            KeyCode::Enter if suggestions.files => {
                app.accept_suggestion(&suggestions);
                app.input_changed();
                return false;
            }
            KeyCode::Enter if !app.busy => app.accept_suggestion(&suggestions),
            KeyCode::Tab => {
                app.accept_suggestion(&suggestions);
                app.command_selected = 0;
                if suggestions.files {
                    app.input_changed();
                }
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
            if app.status == "working" || app.status == "compacting" {
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
        (KeyCode::Up, _) => match row_above(&app.input, app.cursor, app.input_width) {
            Some(cursor) => app.cursor = cursor,
            None => app.scroll_up(1),
        },
        (KeyCode::Down, _) => match row_below(&app.input, app.cursor, app.input_width) {
            Some(cursor) => app.cursor = cursor,
            None => app.scroll_down(1),
        },
        (KeyCode::PageUp, _) => app.scroll_up(app.page_size),
        (KeyCode::PageDown, _) => app.scroll_down(app.page_size),
        (KeyCode::Home, _) => app.scroll_to_top(),
        (KeyCode::End, _) => app.scroll_to_bottom(),
        (KeyCode::Enter, _) if !app.busy && !app.input.trim().is_empty() => {
            app.scroll_to_bottom();
            let prompt = std::mem::take(&mut app.input);
            app.cursor = 0;
            app.files = None;
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
                    "/compact" => {
                        app.start("compacting");
                        act(Action::Compact(app.thinking_level));
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
                        app.start("checking model");
                        act(Action::CheckModel(argument.to_string()));
                    }
                    "/resume" => {
                        app.start("loading sessions");
                        act(Action::ListSessions);
                    }
                    _ if skills::command_name(&prompt).is_some() => {
                        let name = skills::command_name(&prompt).unwrap_or_default();
                        if app.skills.iter().any(|skill| skill.name == name) {
                            app.push(Role::Event, format!("[skill] {name}"));
                            if let Some((_, arguments)) = prompt.split_once(' ')
                                && !arguments.trim().is_empty()
                            {
                                app.push(Role::User, arguments.trim());
                            }
                        } else {
                            app.push(Role::User, prompt.clone());
                        }
                        app.prompt_history.push(prompt.clone());
                        app.start("working");
                        let images = app.take_images(&prompt);
                        act(Action::Submit(prompt, images, app.thinking_level));
                    }
                    command => app.push(Role::Event, format!("unknown command: {command}")),
                }
                return false;
            }
            app.push(Role::User, prompt.clone());
            app.prompt_history.push(prompt.clone());
            app.start("working");
            let images = app.take_images(&prompt);
            act(Action::Submit(prompt, images, app.thinking_level));
        }
        (KeyCode::Left | KeyCode::Char('b'), KeyModifiers::ALT) => {
            app.cursor = previous_word_start(&app.input, app.cursor);
        }
        (KeyCode::Right | KeyCode::Char('f'), KeyModifiers::ALT) => {
            app.cursor = next_word_end(&app.input, app.cursor);
        }
        (KeyCode::Char('v'), KeyModifiers::CONTROL) => act(Action::PasteImage),
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
            app.push_tool(&name, summary, diff);
        }
        UiEvent::Agent(AgentEvent::ToolDone { name, error, note }) => {
            if let Some(error) = error {
                app.push(Role::Event, format!("{name} failed: {error}"));
            }
            if let Some(note) = note {
                app.push(Role::Event, format!("{name}: {note}"));
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
        UiEvent::ModelChecked(result) => app.finish(result, |app, model| app.set_model(&model)),
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
        UiEvent::ImagePasted(Ok(Some(image))) => app.attach_image(image),
        UiEvent::ImagePasted(Ok(None)) => app.push(Role::Event, "no image in the clipboard"),
        UiEvent::ImagePasted(Err(error)) => {
            app.push(Role::Event, format!("failed to paste image: {error}"));
        }
        UiEvent::Resumed(result) => app.finish(result, |app, messages| {
            app.clear_session();
            replay_messages(app, &messages);
            app.push(Role::Event, "resumed session");
        }),
    }
}

fn replay_messages(app: &mut App, messages: &[Value]) {
    let results: std::collections::HashMap<&str, (&str, bool)> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| {
            Some((
                block["tool_use_id"].as_str()?,
                (
                    block["content"].as_str().unwrap_or_default(),
                    block["is_error"] == true,
                ),
            ))
        })
        .collect();
    for message in messages {
        if message["stop_reason"] == "compacted" {
            app.push(Role::Event, "compacted conversation");
            continue;
        }
        let role = message["role"].as_str().unwrap_or_default();
        if let Some(text) = message["content"].as_str() {
            replay_prompt(app, text);
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
                    app.push_tool(
                        name,
                        tools::summary(name, &block["input"]),
                        tools::diff(name, &block["input"]),
                    );
                    match block["id"].as_str().and_then(|id| results.get(id)) {
                        Some((error, true)) => {
                            app.push(Role::Event, format!("{name} failed: {error}"));
                        }
                        Some((result, false)) => {
                            if let Some(note) = tools::note(name, result) {
                                app.push(Role::Event, format!("{name}: {note}"));
                            }
                        }
                        None => {}
                    }
                }
                ("user", "text") => replay_prompt(app, block["text"].as_str().unwrap_or_default()),
                _ => {}
            }
        }
    }
}

fn replay_prompt(app: &mut App, text: &str) {
    match skills::parse_block(text) {
        Some(block) => {
            app.push(Role::Event, format!("[skill] {}", block.name));
            let mut command = format!("/skill:{}", block.name);
            if let Some(user_message) = block.user_message {
                app.push(Role::User, user_message);
                command = format!("{command} {user_message}");
            }
            app.prompt_history.push(command);
        }
        None => {
            app.push(Role::User, text);
            app.prompt_history.push(text.to_string());
        }
    }
}

fn draw(frame: &mut Frame, app: &mut App) {
    let input_width = frame.area().width.max(1) as usize;
    app.input_width = input_width;
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
        Line::raw(format_stats(&app.stats)),
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
    let max_scroll = wrapped_line_count
        .saturating_sub(viewport_height as usize)
        .min(u16::MAX as usize) as u16;
    app.scroll_from_bottom =
        held_scroll_from_bottom(app.scroll_from_bottom, app.max_scroll, max_scroll);
    app.max_scroll = max_scroll;
    app.page_size = viewport_height.max(1);
    let scroll = app.max_scroll.saturating_sub(app.scroll_from_bottom);
    if let Some(picker) = &app.picker {
        frame.render_widget(picker_view(picker, chat.height), chat);
    } else {
        frame.render_widget(conversation.scroll((scroll, 0)), chat);
    }

    if let Some(suggestions) = app.visible_suggestions() {
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
    use super::{
        Action, App, Image, Role, UiEvent, file_matches, file_query, handle_agent_event,
        handle_input, held_scroll_from_bottom, replay_messages,
    };
    use crate::agent::{AgentEvent, Quota, Stats, Usage};
    use crate::skills::Skill;
    use crate::tools;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use serde_json::json;

    #[test]
    fn checks_model_before_selecting_it() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        handle_input(Event::Paste("/model other".into()), &mut app, |_| {});
        let mut checked = None;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut app,
            |action| {
                if let Action::CheckModel(model) = action {
                    checked = Some(model);
                }
            },
        );
        assert_eq!(checked.as_deref(), Some("other"));
        assert_eq!(app.model, "model");
        assert!(app.busy);
    }

    #[test]
    fn ignores_escape_while_loading_models() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
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
            tokens_per_second: None,
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
            tokens_per_second: None,
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
            tokens_per_second: None,
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
            tokens_per_second: None,
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
    fn adds_newline_with_shift_or_alt_enter() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        let mut submitted = false;
        for modifiers in [KeyModifiers::SHIFT, KeyModifiers::ALT] {
            handle_input(Event::Paste("a".into()), &mut app, |_| {});
            handle_input(
                Event::Key(KeyEvent::new(KeyCode::Enter, modifiers)),
                &mut app,
                |_| submitted = true,
            );
        }
        assert!(!submitted);
        assert_eq!(app.input, "a\na\n");
        assert_eq!(app.cursor, app.input.len());
    }

    #[test]
    fn moves_between_input_lines_with_up_and_down() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        app.prompt_history.push("earlier".into());
        handle_input(Event::Paste("one\ntwo".into()), &mut app, |_| {});
        let key = |code| Event::Key(KeyEvent::from(code));
        handle_input(key(KeyCode::Up), &mut app, |_| {});
        assert_eq!(app.cursor, "one".len());
        handle_input(key(KeyCode::Down), &mut app, |_| {});
        assert_eq!(app.cursor, "one\ntwo".len());
        assert_eq!(app.input, "one\ntwo");
        app.input = "abcdefg".into();
        app.cursor = 6;
        app.input_width = 4;
        handle_input(key(KeyCode::Up), &mut app, |_| {});
        assert_eq!(app.cursor, 2);
    }

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
    fn picks_file_after_at_sign() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        app.files = Some(vec!["src/agent.rs".into(), "src/main.rs".into()]);
        for character in "read @src".chars() {
            handle_input(
                Event::Key(KeyEvent::from(KeyCode::Char(character))),
                &mut app,
                |_| {},
            );
        }
        handle_input(Event::Key(KeyEvent::from(KeyCode::Down)), &mut app, |_| {});
        let mut submitted = false;
        handle_input(Event::Key(KeyEvent::from(KeyCode::Enter)), &mut app, |_| {
            submitted = true
        });
        assert!(!submitted);
        assert_eq!(app.input, "read @src/main.rs ");
        assert_eq!(app.cursor, app.input.len());
        assert!(app.visible_suggestions().is_none());
    }

    #[test]
    fn pastes_multiple_lines_without_submitting() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
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
            tokens_per_second: None,
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
    fn sends_pasted_images_whose_markers_remain() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        let mut pasting = false;
        handle_input(
            Event::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL)),
            &mut app,
            |action| pasting = matches!(action, Action::PasteImage),
        );
        assert!(pasting);
        let image = |byte| Image {
            media_type: "image/png",
            data: vec![byte],
        };
        handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(1)))), &mut app);
        handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(2)))), &mut app);
        assert_eq!(app.input, "[image 1][image 2]");
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Backspace)),
            &mut app,
            |_| {},
        );
        handle_input(Event::Paste(" look".into()), &mut app, |_| {});
        let mut sent = None;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut app,
            |action| {
                if let Action::Submit(prompt, images, _) = action {
                    sent = Some((prompt, images));
                }
            },
        );
        assert_eq!(
            sent,
            Some(("[image 1][image 2 look".into(), vec![image(1)]))
        );
        assert!(app.images.is_empty());
    }

    #[test]
    fn keeps_scrolled_view_still_while_content_grows() {
        assert_eq!(held_scroll_from_bottom(0, 10, 15), 0);
        assert_eq!(held_scroll_from_bottom(1, 15, 20), 6);
        assert_eq!(held_scroll_from_bottom(6, 20, 18), 4);
        assert_eq!(held_scroll_from_bottom(1, 15, 10), 0);
    }

    #[test]
    fn merges_consecutive_reads() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        app.push_tool("read", "a.rs".into(), None);
        app.push_tool("read", "b.rs".into(), None);
        app.push_tool("read", "a.rs".into(), None);
        app.push(Role::Event, "read failed: missing");
        app.push_tool("read", "c.rs".into(), None);
        let texts: Vec<&str> = app.messages[1..]
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["read a.rs (2), b.rs", "read failed: missing", "read c.rs"]
        );
    }

    #[test]
    fn replays_failed_tool_calls() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        let read = |id: &str, path: &str| json!({ "type": "tool_use", "id": id, "name": "read", "input": { "path": path } });
        let result = |id: &str, is_error: bool| json!({ "type": "tool_result", "tool_use_id": id, "content": "missing", "is_error": is_error });
        replay_messages(
            &mut app,
            &[
                json!({ "role": "assistant", "content": [read("1", "a.rs"), read("2", "b.rs"), read("3", "c.rs")] }),
                json!({ "role": "user", "content": [result("1", false), result("2", true), result("3", false)] }),
            ],
        );
        let texts: Vec<&str> = app.messages[1..]
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["read a.rs, b.rs", "read failed: missing", "read c.rs"]
        );
    }

    #[test]
    fn replays_compaction_as_event() {
        let stats = Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut app = App::new("model", "medium", stats);
        replay_messages(
            &mut app,
            &[
                json!({ "role": "user", "content": "hello" }),
                json!({ "role": "assistant", "stop_reason": "compacted", "content": [], "summary": "greeted" }),
            ],
        );
        let texts: Vec<&str> = app.messages[1..]
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(texts, ["hello", "compacted conversation"]);
    }

    #[test]
    fn shows_skill_commands_live_and_replayed() {
        let stats = || Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let mut live = App::new("model", "medium", stats());
        live.skills = vec![Skill {
            name: "demo".into(),
            description: "Run\nthe demo.".into(),
            path: "/skills/demo/SKILL.md".into(),
            base_dir: "/skills/demo".into(),
            disable_model_invocation: false,
        }];
        handle_input(Event::Paste("/sk".into()), &mut live, |_| {});
        assert_eq!(
            live.visible_suggestions().unwrap().items,
            [("/skill:demo".to_string(), "Run the demo.".to_string())]
        );
        live.input = "/skill:demo fix it".into();
        let mut submitted = None;
        handle_input(
            Event::Key(KeyEvent::from(KeyCode::Enter)),
            &mut live,
            |action| {
                if let Action::Submit(prompt, _, _) = action {
                    submitted = Some(prompt);
                }
            },
        );
        assert_eq!(submitted.as_deref(), Some("/skill:demo fix it"));
        let mut replayed = App::new("model", "medium", stats());
        replay_messages(
            &mut replayed,
            &[
                json!({ "role": "user", "content": "<skill name=\"demo\" location=\"/skills/demo/SKILL.md\">\nReferences are relative to /skills/demo.\n\nBody\n</skill>\n\nfix it" }),
            ],
        );
        let texts = |app: &App| -> Vec<String> {
            app.messages[1..]
                .iter()
                .map(|message| message.text.clone())
                .collect()
        };
        assert_eq!(texts(&live), ["[skill] demo", "fix it"]);
        assert_eq!(texts(&replayed), texts(&live));
        assert_eq!(replayed.prompt_history, live.prompt_history);
    }

    #[test]
    fn shows_replacement_count_live_and_replayed() {
        let stats = || Stats {
            usage: Usage::default(),
            cache_hit_rate: None,
            tokens_per_second: None,
            context_tokens: 0,
            context_window: 0,
            quota: Quota::default(),
        };
        let input =
            json!({ "path": "a.rs", "old_text": "a", "new_text": "b", "replace_all": true });
        let mut live = App::new("model", "medium", stats());
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolStart {
                name: "edit".into(),
                summary: tools::summary("edit", &input),
                diff: tools::diff("edit", &input),
            }),
            &mut live,
        );
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolDone {
                name: "edit".into(),
                error: None,
                note: Some("3 replacements".into()),
            }),
            &mut live,
        );
        let mut replayed = App::new("model", "medium", stats());
        replay_messages(
            &mut replayed,
            &[
                json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "1", "name": "edit", "input": input }] }),
                json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "1", "content": "edited a.rs (3 replacements)", "is_error": false }] }),
            ],
        );
        let texts = |app: &App| -> Vec<String> {
            app.messages[1..]
                .iter()
                .map(|message| message.text.clone())
                .collect()
        };
        assert_eq!(texts(&live), ["edit a.rs\n-a\n+b", "edit: 3 replacements"]);
        assert_eq!(texts(&replayed), texts(&live));
    }
}
