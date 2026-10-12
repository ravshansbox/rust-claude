mod app;
mod commands;
mod draw;
mod files;
mod input;
mod keys;
mod mcp_status;
mod message;
mod prompt_history;
mod render;
mod replay;
mod selection;
mod status;
mod tasks;
mod terminal;
#[cfg(test)]
mod test_support;
mod worker;

use crate::{
    StopSignals,
    agent::{Agent, AgentEvent},
    clipboard,
    highlight::Colour,
    history, images, mcp,
    settings::Settings,
    skills::{Scope, Skills},
    tools,
    update::Updater,
};
use anyhow::Result;
use app::{Activity, App, Picker, PickerKind};
use crossterm::event::EventStream;
use draw::draw;
use files::list_files;
use futures::StreamExt;
use keys::{Action, handle_input, may_change_screen};
use message::Role;
use prompt_history::HistorySearch;
use ratatui::DefaultTerminal;
use render::{BACKGROUND, THEME, Theme};
use replay::{handle_agent_event, replay_messages};
use std::{
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime},
};
use terminal::{HeldPanics, InputModes, notify};
use tokio::{
    sync::mpsc,
    time::{MissedTickBehavior, interval},
};

/// Opens `url` in the default browser, if there is one.
fn open_in_browser(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let child = tokio::process::Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

/// Signs in to the MCP server `name` in the browser, then starts it again.
async fn sign_in_to_mcp_server(
    name: String,
    events: mpsc::UnboundedSender<UiEvent>,
    requests: mpsc::UnboundedSender<Request>,
) {
    let notice = |text: String| {
        let _ = events.send(UiEvent::Agent(AgentEvent::Notice(text)));
    };
    let sign_in = match mcp::begin_sign_in(&name).await {
        Ok(sign_in) => sign_in,
        Err(error) => return notice(format!("MCP sign-in to {name} failed: {error}")),
    };
    notice(format!(
        "opening the sign-in page for MCP server {name}; if it does not open, visit {}",
        sign_in.authorize_url
    ));
    open_in_browser(&sign_in.authorize_url);
    if let Err(error) = sign_in.finish().await {
        return notice(format!("MCP sign-in to {name} failed: {error}"));
    }
    let signed_in = format!("signed in to MCP server {name}");
    restart_mcp_server(name, signed_in, events, requests).await;
}

/// Forgets the sign-in for the MCP server `name`, then starts it again.
async fn sign_out_of_mcp_server(
    name: String,
    events: mpsc::UnboundedSender<UiEvent>,
    requests: mpsc::UnboundedSender<Request>,
) {
    match mcp::sign_out(&name) {
        Ok(signed_out) => restart_mcp_server(name, signed_out, events, requests).await,
        Err(error) => {
            let notice = format!("MCP sign-out of {name} failed: {error}");
            let _ = events.send(UiEvent::Agent(AgentEvent::Notice(notice)));
        }
    }
}

/// Shows `notice` and starts the MCP server `name` again in place of the
/// running one.
async fn restart_mcp_server(
    name: String,
    notice: String,
    events: mpsc::UnboundedSender<UiEvent>,
    requests: mpsc::UnboundedSender<Request>,
) {
    let Some(server) = mcp::restart(&name) else {
        let _ = events.send(UiEvent::Agent(AgentEvent::Notice(notice)));
        return;
    };
    let _ = events.send(UiEvent::McpRestarting {
        name,
        scope: server.scope,
        notice,
    });
    let _ = requests.send(Request::ReplaceMcpServer(server.started.await));
}
use worker::{Request, UiEvent, agent_task, quit};

pub(crate) use render::{ansi_thinking, ansi_tool_lines, tool_message};
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
    let mut app = App::new(&agent.model, agent.effort, agent.stats());
    app.skills = agent.skills.skills.clone();
    let startup = mcp::startup();
    app.mcp_sign_in_servers = startup.sign_in_servers;
    for diagnostic in startup.diagnostics {
        app.push(Role::Event, diagnostic);
    }
    app.load_history(history::history_path());
    if !agent.messages().is_empty() {
        replay_messages(&mut app, agent.messages());
        app.push(Role::Event, "continued session");
    }
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let background_events = event_tx.clone();
    let (history_sender, history_writer) = history_writer(event_tx.clone());
    app.queue = agent.queue.clone();
    let (mcp_tx, mcp_rx) = mpsc::unbounded_channel();
    app.push(Role::Event, tools_line());
    app.start_mcp_servers(
        startup
            .servers
            .iter()
            .map(|server| (server.name.as_str(), server.scope)),
    );
    app.show_overridden_mcp_servers(startup.overridden.iter().map(String::as_str));
    show_skills_and_instructions(
        &mut app,
        &agent.skills,
        agent
            .instructions
            .iter()
            .map(|instructions| instructions.label.as_str()),
    );
    for server in startup.servers {
        let mcp_tx = mcp_tx.clone();
        tokio::spawn(async move {
            let _ = mcp_tx.send(server.started.await);
        });
    }
    drop(mcp_tx);
    let settings = Settings::load();
    app.notifications = settings.notifications != Some(false);
    if let Some(updater) = Updater::for_this_build(&settings) {
        let events = background_events.clone();
        tokio::spawn(async move {
            updater
                .run(move |progress| {
                    let _ = events.send(UiEvent::Update(progress));
                })
                .await;
        });
    }
    let mut worker = tokio::spawn(agent_task(agent, request_rx, cancel_rx, mcp_rx, event_tx));
    let mut terminal_events = EventStream::new();
    let mut redraw = interval(REDRAW_INTERVAL);
    redraw.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut spinner = interval(SPINNER_INTERVAL);
    spinner.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut dirty = true;
    let mut saved_model = app.model.clone();
    let mut saved_effort = app.effort;
    let mut sent_effort = app.effort;

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
                let send = |request| {
                    let _ = request_tx.send(request);
                };
                let quit = handle_input(event, &mut app, |action| match action {
                    Action::Submit(prompt, images, effort) => {
                        send(Request::Prompt(prompt, images, effort));
                    }
                    Action::Shell(command) => send(Request::Shell(command)),
                    Action::PasteImage => {
                        let events = background_events.clone();
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
                            background_events.clone(),
                            request_tx.clone(),
                        ));
                    }
                    Action::McpSignOut(name) => {
                        tokio::spawn(sign_out_of_mcp_server(
                            name,
                            background_events.clone(),
                            request_tx.clone(),
                        ));
                    }
                    Action::Cancel => {
                        let _ = cancel_tx.send(());
                    }
                });
                if app.effort != sent_effort {
                    sent_effort = app.effort;
                    let _ = request_tx.send(Request::SetEffort(sent_effort));
                }
                save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
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
                if !app.busy()
                    && let Some((prompt, images)) = app.send_queued()
                {
                    let _ = request_tx.send(Request::Prompt(prompt, images, app.effort));
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
                save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
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
        for entry in app.unsaved_history.drain(..) {
            let _ = history_sender.send(entry);
        }
    };
    for entry in app.unsaved_history.drain(..) {
        let _ = history_sender.send(entry);
    }
    drop(history_sender);
    let _ = history_writer.join();
    quit(request_tx, cancel_tx);
    if !worker_stopped {
        let _ = worker.await;
    }
    result
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
                let _ = events.send(UiEvent::Agent(AgentEvent::Notice(notice)));
            }
        }
    });
    (sender, writer)
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

fn show_skills_and_instructions<'a>(
    app: &mut App,
    skills: &Skills,
    instructions: impl IntoIterator<Item = &'a str>,
) {
    for scope in [Scope::Global, Scope::Project] {
        let names: Vec<&str> = skills
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
    for diagnostic in &skills.diagnostics {
        app.push(Role::Event, diagnostic.to_string());
    }
    let labels: Vec<&str> = instructions.into_iter().collect();
    if !labels.is_empty() {
        app.push(Role::Event, format!("loaded {}", labels.join(", ")));
    }
}

fn tools_line() -> String {
    let definitions = tools::definitions();
    let mut names: Vec<&str> = definitions
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|definition| definition["name"].as_str())
        .collect();
    names.sort_unstable();
    format!("loaded tools: {}", names.join(", "))
}

fn display_model(model: &str) -> &str {
    model.strip_prefix("claude-").unwrap_or(model)
}

#[cfg(test)]
mod tests {
    use super::{
        PathBuf, UiEvent, git_branch, history_writer, show_skills_and_instructions,
        test_support::new_app, tools_line,
    };
    use crate::{
        agent::AgentEvent,
        history,
        skills::{Diagnostic, Scope, Skill, Skills},
        tools::test_support::TemporaryDir,
    };

    #[test]
    fn lists_loaded_skills_by_scope_then_their_problems_then_instructions() {
        let skill = |name: &str, scope| Skill {
            name: name.into(),
            description: String::new(),
            path: PathBuf::new(),
            base_dir: PathBuf::new(),
            disable_model_invocation: false,
            scope,
        };
        let skills = Skills {
            skills: vec![
                skill("review", Scope::Project),
                skill("commit", Scope::Global),
                skill("deploy", Scope::Project),
            ],
            diagnostics: vec![Diagnostic::Warning {
                path: "broken/SKILL.md".into(),
                message: "missing description".into(),
            }],
        };
        let mut app = new_app();
        show_skills_and_instructions(&mut app, &skills, ["~/AGENTS.md", "./AGENTS.md"]);
        let shown: Vec<&str> = app.messages[1..]
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            shown,
            [
                "loaded global skills: commit",
                "loaded project skills: review, deploy",
                "skill warning: broken/SKILL.md: missing description",
                "loaded ~/AGENTS.md, ./AGENTS.md",
            ]
        );
        let mut app = new_app();
        show_skills_and_instructions(&mut app, &Skills::default(), []);
        assert_eq!(app.messages.len(), 1);
    }

    #[test]
    fn shows_the_branch_of_a_repository_with_no_commits() {
        let directory = TemporaryDir::new("unborn-branch");
        let status = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(directory.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(git_branch(directory.path()).as_deref(), Some("main"));
    }

    #[test]
    fn lists_built_in_tools_on_one_line() {
        assert_eq!(
            tools_line(),
            "loaded tools: agent, bash, bash_stop, edit, read, web_search, write"
        );
    }

    #[test]
    fn saves_prompt_history_in_order_and_reports_failures() {
        let directory = TemporaryDir::new("history-writer");
        let folder = directory.path();
        let path = folder.join("history.jsonl");
        std::fs::write(folder.join("missing"), "a file, not a folder").unwrap();
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (sender, writer) = history_writer(events);
        for prompt in ["one", "two"] {
            sender
                .send((path.clone(), history::Entry::here(prompt)))
                .unwrap();
        }
        sender
            .send((
                folder.join("missing").join("history.jsonl"),
                history::Entry::here("three"),
            ))
            .unwrap();
        drop(sender);
        writer.join().unwrap();
        let saved: Vec<String> = history::load(&path)
            .into_iter()
            .map(|entry| entry.prompt)
            .collect();
        let notice = match received.try_recv() {
            Ok(UiEvent::Agent(AgentEvent::Notice(notice))) => notice,
            _ => String::new(),
        };
        assert_eq!(saved, ["one", "two"]);
        assert!(
            notice.starts_with("failed to save prompt history: "),
            "{notice}"
        );
    }
}
