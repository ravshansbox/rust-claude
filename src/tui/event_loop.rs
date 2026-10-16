use super::{
    App,
    background::{start_background_work, start_mcp_servers, start_updater},
    draw::draw,
    keys::{handle_input, may_change_screen},
    opening::opening_app,
    perform::perform,
    prompt_history::{finish_history, history_writer},
    replay::handle_agent_event,
    settings_sync::{save_changed_settings, send_changed_effort},
    terminal::notify,
    worker::{Request, UiEvent, agent_task, quit},
};
use crate::{
    agent::Agent, config, history, mcp, restart::Restart, settings::Settings, stop::StopSignals,
};
use anyhow::Result;
use crossterm::event::EventStream;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use std::time::Duration;
use tokio::{
    sync::mpsc,
    task::JoinError,
    time::{Interval, MissedTickBehavior, interval},
};

const REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

pub(super) async fn run_loop(
    terminal: &mut DefaultTerminal,
    agent: Agent,
    mut stop: StopSignals,
    restarted_input: Option<String>,
) -> Result<Option<Restart>> {
    let startup = mcp::startup();
    let mut app = opening_app(&agent, &startup, history::history_path());
    if let Some(input) = restarted_input {
        app.restored_after_update(input);
    }
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
    let mut restart_program = None;
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
        restart_program = app.restart_due();
        if restart_program.is_some() {
            break Ok(());
        }
    };
    finish_history(&mut app, history_sender, history_writer);
    quit(request_tx, cancel_tx);
    let session = if worker_stopped {
        None
    } else {
        worker.await.ok().flatten()
    };
    result?;
    Ok(restart_program.map(|program| Restart {
        program,
        session,
        model: app.model.clone(),
        effort: app.effort,
        input: std::mem::take(&mut app.input),
        config_dir: config::dir(),
    }))
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

fn worker_error<T>(joined: Result<T, JoinError>) -> anyhow::Error {
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
