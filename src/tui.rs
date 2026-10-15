mod app;
mod background;
mod commands;
mod draw;
mod files;
mod input;
mod keys;
mod mcp_sign_in;
mod mcp_status;
mod message;
mod opening;
mod perform;
mod prompt_history;
mod render;
mod replay;
mod selection;
mod settings_sync;
mod status;
mod suggestions;
mod tasks;
mod terminal;
#[cfg(test)]
mod test_support;
mod worker;
mod workspace;

use crate::{agent::Agent, highlight::Colour, history, mcp, settings::Settings, stop::StopSignals};
use anyhow::Result;
use app::{Activity, App, Picker, PickerKind};
use background::{start_background_work, start_mcp_servers, start_updater};
use crossterm::event::EventStream;
use draw::draw;
use futures::StreamExt;
use keys::{handle_input, may_change_screen};
use message::Role;
use opening::opening_app;
use perform::perform;
use prompt_history::{HistorySearch, finish_history, history_writer};
use ratatui::DefaultTerminal;
use render::{BACKGROUND, THEME, Theme};
use replay::handle_agent_event;
use settings_sync::{save_changed_settings, send_changed_effort};
use std::{
    io,
    time::{Duration, SystemTime},
};
use terminal::{HeldPanics, InputModes, notify};
use tokio::{
    sync::mpsc,
    task::JoinError,
    time::{Interval, MissedTickBehavior, interval},
};
use worker::{Request, UiEvent, agent_task, quit, send_notice};
use workspace::workspace_label;

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

fn display_model(model: &str) -> &'static str {
    crate::models::name(model)
}

#[cfg(test)]
mod tests;
