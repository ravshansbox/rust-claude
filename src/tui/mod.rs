mod app;
mod commands;
mod draw;
mod files;
mod input;
mod keys;
mod render;
mod replay;
mod status;
#[cfg(test)]
mod test_support;
mod worker;

use crate::{
    StopSignals, agent::Agent, clipboard, history, images, mcp, settings::Settings, skills::Scope,
};
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
use draw::draw;
use files::list_files;
use futures::StreamExt;
use keys::{Action, handle_input};
use ratatui::DefaultTerminal;
use render::{THEME, Theme};
use replay::{handle_agent_event, replay_messages};
use std::{
    io::Write,
    time::{Duration, SystemTime},
};
use tokio::{sync::mpsc, time::MissedTickBehavior};
use worker::{Request, UiEvent, agent_task, quit};

pub fn dark_theme() -> bool {
    matches!(Theme::detect(), Theme::Dark)
}

const REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

/// Mouse capture, bracketed paste and keyboard flags, turned off when dropped
/// so a panic does not leave them on in the user's shell.
struct InputModes<W: Write> {
    output: W,
    keyboard_enhanced: bool,
}

impl<W: Write> InputModes<W> {
    fn enable(mut output: W) -> std::io::Result<Self> {
        execute!(output, EnableMouseCapture, EnableBracketedPaste)?;
        let keyboard_enhanced = execute!(
            output,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        Ok(Self {
            output,
            keyboard_enhanced,
        })
    }
}

impl<W: Write> Drop for InputModes<W> {
    fn drop(&mut self) {
        if self.keyboard_enhanced {
            let _ = execute!(self.output, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(self.output, DisableMouseCapture, DisableBracketedPaste);
    }
}

pub async fn run(agent: Agent, stop: StopSignals) -> Result<()> {
    THEME.get_or_init(Theme::detect);
    let mut terminal = ratatui::init();
    let modes = match InputModes::enable(std::io::stdout()) {
        Ok(modes) => modes,
        Err(error) => {
            ratatui::restore();
            return Err(error.into());
        }
    };
    let result = run_loop(&mut terminal, agent, stop).await;
    drop(modes);
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut DefaultTerminal,
    agent: Agent,
    mut stop: StopSignals,
) -> Result<()> {
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
    let startup = mcp::startup();
    for diagnostic in &startup.diagnostics {
        app.push(Role::Event, diagnostic.to_string());
    }
    for program in &agent.missing_programs {
        app.push(Role::Event, format!("{program} not found on PATH"));
    }
    app.greet();
    app.load_history(history::history_path());
    if !agent.messages().is_empty() {
        replay_messages(&mut app, agent.messages());
        app.push(Role::Event, "continued session");
    }
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let background_events = event_tx.clone();
    app.queue = agent.queue.clone();
    let (mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    for server in startup.servers {
        let mcp_tx = mcp_tx.clone();
        tokio::spawn(async move {
            let _ = mcp_tx.send(server.await);
        });
    }
    drop(mcp_tx);
    let mut worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    let mut terminal_events = EventStream::new();
    let mut redraw = tokio::time::interval(REDRAW_INTERVAL);
    redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut spinner = tokio::time::interval(SPINNER_INTERVAL);
    spinner.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;
    let mut saved_model = app.model.clone();
    let mut saved_thinking_level = app.thinking_level;

    let mut worker_stopped = false;
    let result = loop {
        tokio::select! {
            joined = &mut worker => {
                worker_stopped = true;
                break Err(match joined {
                    Err(error) if error.is_panic() => anyhow::anyhow!("the agent crashed"),
                    _ => anyhow::anyhow!("the agent stopped unexpectedly"),
                });
            }
            _ = stop.recv() => break Ok(()),
            _ = redraw.tick(), if dirty => {
                if let Err(error) = terminal.draw(|frame| draw(frame, &mut app)) {
                    break Err(error.into());
                }
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
                        let events = background_events.clone();
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
        if let Some(generation) = app.start_listing_files() {
            let events = background_events.clone();
            tokio::task::spawn_blocking(move || {
                let _ = events.send(UiEvent::Files(generation, list_files()));
            });
        }
        if std::mem::take(&mut app.workspace_stale) {
            let events = background_events.clone();
            tokio::task::spawn_blocking(move || {
                let _ = events.send(UiEvent::Workspace(workspace_label()));
            });
        }
    };
    quit(request_tx, cancel_tx);
    if !worker_stopped {
        let _ = worker.await;
    }
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
    let model = (app.model != *saved_model).then(|| app.model.clone());
    let thinking_level =
        (app.thinking_level != *saved_thinking_level).then_some(app.thinking_level);
    saved_model.clone_from(&app.model);
    *saved_thinking_level = app.thinking_level;
    let saved = Settings::update(|settings| {
        if let Some(model) = model {
            settings.model = Some(model);
        }
        if let Some(thinking_level) = thinking_level {
            settings.thinking_level = Some(thinking_level.to_string());
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
    use super::InputModes;

    #[test]
    fn turns_input_modes_off_after_a_panic() {
        let mut output = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _modes = InputModes::enable(&mut output).unwrap();
            panic!("drawing failed");
        }));
        assert!(result.is_err());
        let written = String::from_utf8_lossy(&output);
        let (enabled, disabled) = written.split_at(written.find("\x1b[<1u").unwrap());
        assert!(enabled.contains("\x1b[?1000h") && enabled.contains("\x1b[?2004h"));
        assert!(
            disabled.contains("\x1b[?1000l"),
            "mouse capture still on: {written:?}"
        );
        assert!(
            disabled.contains("\x1b[?2004l"),
            "bracketed paste still on: {written:?}"
        );
    }
}
