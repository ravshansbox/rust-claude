mod app;
mod commands;
mod draw;
mod files;
mod input;
mod keys;
mod mcp_sign_in;
mod mcp_status;
mod message;
mod opening;
mod prompt_history;
mod render;
mod replay;
mod selection;
mod status;
mod suggestions;
mod tasks;
mod terminal;
#[cfg(test)]
mod test_support;
mod worker;

use crate::{
    StopSignals, agent::Agent, clipboard, highlight::Colour, history, images, mcp,
    settings::Settings, update::Updater,
};
use anyhow::Result;
use app::{Activity, App, Picker, PickerKind};
use crossterm::event::EventStream;
use draw::draw;
use files::list_files;
use futures::StreamExt;
use keys::{Action, handle_input, may_change_screen};
use mcp_sign_in::{sign_in_to_mcp_server, sign_out_of_mcp_server};
use message::Role;
use opening::opening_app;
use prompt_history::HistorySearch;
use ratatui::DefaultTerminal;
use render::{BACKGROUND, THEME, Theme};
use replay::handle_agent_event;
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use terminal::{HeldPanics, InputModes, notify};
use tokio::{
    sync::mpsc,
    task::JoinError,
    time::{Interval, MissedTickBehavior, interval},
};
use worker::{Request, UiEvent, agent_task, quit, send_notice};

pub(crate) use render::{ansi_thinking, ansi_tool_lines, tool_failure, tool_message, tool_note};
pub(crate) use tasks::finished_task;

/// Whether the terminal has a dark theme, and its background if it says.
pub fn terminal_theme() -> (bool, Option<Colour>) {
    let (theme, background) = Theme::detect();
    (matches!(theme, Theme::Dark), background)
}

const REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

pub async fn run(agent: Agent, stop: StopSignals) -> Result<()> {
    let (theme, background) = Theme::detect();
    THEME.get_or_init(|| theme);
    BACKGROUND.get_or_init(|| background);
    let mut terminal = ratatui::init();
    let held = HeldPanics::install(io::stderr());
    let modes = match InputModes::enable(io::stdout()) {
        Ok(modes) => modes,
        Err(error) => {
            ratatui::restore();
            drop(held);
            return Err(error.into());
        }
    };
    let result = run_loop(&mut terminal, agent, stop).await;
    drop(modes);
    ratatui::restore();
    drop(held);
    result
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    agent: Agent,
    mut stop: StopSignals,
) -> Result<()> {
    let startup = mcp::startup();
    let mut app = opening_app(&agent, &startup, history::history_path());
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let background_events = event_tx.clone();
    let (history_sender, history_writer) = history_writer(event_tx.clone());
    let mcp_rx = start_mcp_servers(startup.servers);
    let settings = Settings::load();
    app.notifications = settings.notifications != Some(false);
    start_updater(&settings, &background_events);
    let mut worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    let mut terminal_events = EventStream::new();
    let mut redraw = skipping_interval(REDRAW_INTERVAL);
    let mut spinner = skipping_interval(SPINNER_INTERVAL);
    let mut dirty = true;
    let mut saved_model = app.model.clone();
    let mut saved_effort = app.effort;
    let mut sent_effort = app.effort;

    let mut worker_stopped = false;
    let result = loop {
        tokio::select! {
            joined = &mut worker => {
                worker_stopped = true;
                break Err(worker_error(joined));
            }
            _ = stop.recv() => break Ok(()),
            _ = redraw.tick(), if dirty => {
                if let Err(error) = terminal.draw(|frame| draw(frame, &mut app)) {
                    break Err(error.into());
                }
                dirty = false;
            }
            _ = spinner.tick(), if app.animating() => {
                app.tick_spinner();
                dirty = true;
            }
            event = terminal_events.next() => {
                let event = match event {
                    Some(Ok(event)) => event,
                    Some(Err(error)) => break Err(error.into()),
                    None => break Ok(()),
                };
                dirty |= may_change_screen(&event);
                let quit = handle_input(event, &mut app, |action| {
                    perform(action, &request_tx, &cancel_tx, &background_events);
                });
                send_changed_effort(&app, &mut sent_effort, &request_tx);
                save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
                if quit {
                    break Ok(());
                }
            }
            Some(event) = event_rx.recv() => {
                dirty = true;
                handle_agent_events(event, &mut event_rx, &mut app);
                send_queued_and_notify(&mut app, terminal, &request_tx);
                save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
            }
        }
        start_background_work(&mut app, &background_events, &history_sender);
    };
    finish_history(&mut app, history_sender, history_writer);
    quit(request_tx, cancel_tx);
    if !worker_stopped {
        let _ = worker.await;
    }
    result
}

fn skipping_interval(period: Duration) -> Interval {
    let mut ticks = interval(period);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticks
}

fn handle_agent_events(
    first: UiEvent,
    events: &mut mpsc::UnboundedReceiver<UiEvent>,
    app: &mut App,
) {
    let mut next = Some(first);
    while let Some(event) = next {
        handle_agent_event(event, app);
        next = events.try_recv().ok();
    }
}

fn worker_error(joined: Result<(), JoinError>) -> anyhow::Error {
    match joined {
        Err(error) if error.is_panic() => anyhow::anyhow!("the agent crashed"),
        _ => anyhow::anyhow!("the agent stopped unexpectedly"),
    }
}

fn send_queued_and_notify(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    requests: &mpsc::UnboundedSender<Request>,
) {
    if !app.busy()
        && let Some((prompt, images)) = app.send_queued()
    {
        let _ = requests.send(Request::Prompt(prompt, images, app.effort));
    }
    if let Some(message) = app.notification.take()
        && !app.busy()
    {
        let _ = notify(
            terminal.backend_mut(),
            &message,
            std::env::var_os("TMUX").is_some(),
        );
    }
}

fn start_mcp_servers(servers: Vec<mcp::StartingServer>) -> mpsc::UnboundedReceiver<mcp::Started> {
    let (mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    for server in servers {
        let mcp_tx = mcp_tx.clone();
        tokio::spawn(async move {
            let _ = mcp_tx.send(server.started.await);
        });
    }
    mcp_rx
}

fn start_updater(settings: &Settings, events: &mpsc::UnboundedSender<UiEvent>) {
    if let Some(updater) = Updater::for_this_build(settings) {
        let events = events.clone();
        tokio::spawn(async move {
            updater
                .run(move |progress| {
                    let _ = events.send(UiEvent::Update(progress));
                })
                .await;
        });
    }
}

fn perform(
    action: Action,
    requests: &mpsc::UnboundedSender<Request>,
    cancel: &mpsc::UnboundedSender<()>,
    events: &mpsc::UnboundedSender<UiEvent>,
) {
    let send = |request| {
        let _ = requests.send(request);
    };
    match action {
        Action::Submit(prompt, images, effort) => send(Request::Prompt(prompt, images, effort)),
        Action::Shell(command) => send(Request::Shell(command)),
        Action::PasteImage => {
            let events = events.clone();
            tokio::spawn(async move {
                let result = clipboard::read_image()
                    .await
                    .and_then(|data| data.map(images::prepare).transpose());
                let _ = events.send(UiEvent::ImagePasted(result));
            });
        }
        Action::Compact(effort) => send(Request::Compact(effort)),
        Action::ListSessions => send(Request::ListSessions),
        Action::Resume(id) => send(Request::Resume(id)),
        Action::SetModel(model) => send(Request::SetModel(model)),
        Action::NewSession => send(Request::NewSession),
        Action::McpSignIn(name) => {
            tokio::spawn(sign_in_to_mcp_server(
                name,
                events.clone(),
                requests.clone(),
            ));
        }
        Action::McpSignOut(name) => {
            tokio::spawn(sign_out_of_mcp_server(
                name,
                events.clone(),
                requests.clone(),
            ));
        }
        Action::Cancel => {
            let _ = cancel.send(());
        }
    }
}

fn start_background_work(
    app: &mut App,
    events: &mpsc::UnboundedSender<UiEvent>,
    history: &std::sync::mpsc::Sender<(PathBuf, history::Entry)>,
) {
    if let Some(generation) = app.start_listing_files() {
        send_blocking(events, move || UiEvent::Files(generation, list_files()));
    }
    if std::mem::take(&mut app.workspace_stale) {
        send_blocking(events, || UiEvent::Workspace(workspace_label()));
    }
    send_history(app, history);
}

fn send_blocking(
    events: &mpsc::UnboundedSender<UiEvent>,
    event: impl FnOnce() -> UiEvent + Send + 'static,
) {
    let events = events.clone();
    tokio::task::spawn_blocking(move || {
        let _ = events.send(event());
    });
}

fn send_history(app: &mut App, sender: &std::sync::mpsc::Sender<(PathBuf, history::Entry)>) {
    for entry in app.unsaved_history.drain(..) {
        let _ = sender.send(entry);
    }
}

fn finish_history(
    app: &mut App,
    sender: std::sync::mpsc::Sender<(PathBuf, history::Entry)>,
    writer: std::thread::JoinHandle<()>,
) {
    send_history(app, &sender);
    drop(sender);
    let _ = writer.join();
}

/// Adds prompts to their history files one at a time, in order, on a thread
/// of its own, so a lock held by another copy does not hold up the interface.
/// It finishes the prompts sent to it once the sender is dropped.
fn history_writer(
    events: mpsc::UnboundedSender<UiEvent>,
) -> (
    std::sync::mpsc::Sender<(PathBuf, history::Entry)>,
    std::thread::JoinHandle<()>,
) {
    let (sender, entries) = std::sync::mpsc::channel::<(PathBuf, history::Entry)>();
    let writer = std::thread::spawn(move || {
        for (path, entry) in entries {
            if let Err(error) = history::append(&path, &entry) {
                let notice = format!("failed to save prompt history: {error:#}");
                send_notice(&events, notice);
            }
        }
    });
    (sender, writer)
}

fn send_changed_effort(
    app: &App,
    sent_effort: &mut &'static str,
    requests: &mpsc::UnboundedSender<Request>,
) {
    if app.effort != *sent_effort {
        *sent_effort = app.effort;
        let _ = requests.send(Request::SetEffort(app.effort));
    }
}

fn save_changed_settings(app: &mut App, saved_model: &mut String, saved_effort: &mut &'static str) {
    if app.model == *saved_model && app.effort == *saved_effort {
        return;
    }
    let model = (app.model != *saved_model).then(|| app.model.clone());
    let effort = (app.effort != *saved_effort).then_some(app.effort);
    saved_model.clone_from(&app.model);
    *saved_effort = app.effort;
    let saved = Settings::update(|settings| {
        if let Some(model) = model {
            settings.model = Some(model);
        }
        if let Some(effort) = effort {
            settings.effort = Some(effort.to_string());
        }
    });
    if let Err(error) = saved {
        app.push(Role::Event, format!("failed to save settings: {error:#}"));
    }
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
    match git_branch(Path::new(".")) {
        Some(branch) => format!("{folder} · {branch}"),
        None => folder,
    }
}

fn git_branch(dir: &Path) -> Option<String> {
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|branch| !branch.is_empty())
    };
    git(&["symbolic-ref", "--short", "-q", "HEAD"])
        .or_else(|| git(&["rev-parse", "--abbrev-ref", "HEAD"]))
}

fn display_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

#[cfg(test)]
mod tests;
