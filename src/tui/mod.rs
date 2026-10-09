mod app;
mod commands;
mod draw;
mod files;
mod input;
mod keys;
mod question;
mod render;
mod replay;
mod status;
#[cfg(test)]
mod test_support;
mod worker;

use crate::{agent::Agent, clipboard, history, images, settings::Settings, skills::Scope};
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
use futures::StreamExt;
use keys::{Action, handle_input};
use ratatui::DefaultTerminal;
use render::{THEME, Theme};
use replay::{handle_agent_event, replay_messages};
use std::time::{Duration, SystemTime};
use tokio::{sync::mpsc, time::MissedTickBehavior};
use worker::{Request, UiEvent, agent_task};

pub fn dark_theme() -> bool {
    matches!(Theme::detect(), Theme::Dark)
}

const REDRAW_INTERVAL: Duration = Duration::from_millis(16);
const SPINNER_INTERVAL: Duration = Duration::from_millis(80);

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

async fn run_loop(terminal: &mut DefaultTerminal, mut agent: Agent) -> Result<()> {
    agent.ask_user = true;
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
    for loaded in agent.mcp.loaded() {
        app.push(Role::Event, loaded);
    }
    for diagnostic in &agent.mcp.diagnostics {
        app.push(Role::Event, diagnostic.to_string());
    }
    for program in &agent.missing_programs {
        app.push(Role::Event, format!("{program} not found on PATH"));
    }
    app.push(
        Role::Event,
        "Ask me to inspect, explain, or edit this project.",
    );
    if !agent.messages().is_empty() {
        replay_messages(&mut app, agent.messages());
        app.push(Role::Event, "continued session");
    }
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let (cancel_tx, cancel_rx) = mpsc::unbounded_channel();
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    let image_events = event_tx.clone();
    app.queue = agent.queue.clone();
    app.history_file = history::history_path();
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
                    Action::Shell(command) => {
                        let _ = request_tx.send(Request::Shell(command));
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
    use super::test_support::{app_with_reply, new_app, press, screen, type_text};
    use super::{App, UiEvent, handle_agent_event};
    use crate::agent::AgentEvent;
    use crate::ask;
    use crate::tui::files::{file_matches, file_query};
    use crossterm::event::KeyCode;
    use serde_json::json;
    use tokio::sync::oneshot::{self, error::TryRecvError};

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

    fn ask(app: &mut App, input: serde_json::Value) -> oneshot::Receiver<Vec<Vec<String>>> {
        let (reply, answers) = oneshot::channel();
        app.busy = true;
        handle_agent_event(
            UiEvent::Agent(AgentEvent::Question {
                questions: ask::parse(&input).unwrap(),
                reply,
            }),
            app,
        );
        answers
    }

    fn output_question(multi_select: bool) -> serde_json::Value {
        json!({ "questions": [{
            "question": "Which output?",
            "header": "Output",
            "multi_select": multi_select,
            "options": [
                { "label": "JSON", "description": "Structured", "recommended": true },
                { "label": "Text", "description": "Readable" }
            ]
        }] })
    }

    #[test]
    fn shows_a_question_and_sends_the_chosen_option() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        let shown = screen(&mut app);
        assert!(shown.contains("Output: Which output?"), "{shown}");
        assert!(shown.contains("JSON (recommended): Structured"), "{shown}");
        assert!(shown.contains("Text: Readable"), "{shown}");
        assert!(shown.contains("Other: "), "{shown}");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv().unwrap(), vec![vec!["Text".to_string()]]);
        assert!(!screen(&mut app).contains("Which output?"));
    }

    #[test]
    fn sends_a_typed_answer_for_other() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        type_text(&mut app, "YAML please");
        assert!(screen(&mut app).contains("Other: YAML please"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["Other: YAML please".to_string()]]
        );
        assert_eq!(app.input, "");
    }

    #[test]
    fn ignores_enter_on_an_empty_other() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Empty));
        assert!(screen(&mut app).contains("Which output?"));
    }

    #[test]
    fn toggles_options_in_a_multiple_choice_question() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(true));
        press(&mut app, KeyCode::Char(' '));
        assert!(screen(&mut app).contains("[x] JSON"));
        assert!(screen(&mut app).contains("[ ] Text"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        type_text(&mut app, "a b");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["JSON".to_string(), "Other: a b".to_string()]]
        );
    }

    #[test]
    fn asks_each_question_in_turn() {
        let mut app = new_app();
        let mut input = output_question(false);
        let mut second = input["questions"][0].clone();
        second["question"] = json!("Which colour?");
        second["header"] = json!("Colour");
        input["questions"].as_array_mut().unwrap().push(second);
        let mut answers = ask(&mut app, input);
        assert!(screen(&mut app).contains("Output (1/2): Which output?"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Empty));
        assert!(screen(&mut app).contains("Colour (2/2): Which colour?"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["JSON".to_string()], vec!["Text".to_string()]]
        );
    }

    #[test]
    fn declines_the_question_on_escape() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Esc);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Closed));
        assert!(!screen(&mut app).contains("Which output?"));
        assert!(app.busy);
    }

    #[test]
    fn keeps_the_conversation_visible_while_asking() {
        let mut app = app_with_reply();
        ask(&mut app, output_question(false));
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("Which output?"), "{shown}");
    }

    #[test]
    fn highlights_the_recommended_option_even_when_listed_later() {
        let mut app = new_app();
        let mut answers = ask(
            &mut app,
            json!({ "questions": [{
                "question": "Which output?",
                "header": "Output",
                "options": [
                    { "label": "Text", "description": "Readable" },
                    { "label": "JSON", "description": "Structured", "recommended": true }
                ]
            }] }),
        );
        let shown = screen(&mut app);
        let json_row = shown.find("JSON (recommended)").unwrap();
        let text_row = shown.find("Text: Readable").unwrap();
        assert!(json_row < text_row, "{shown}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv().unwrap(), vec![vec!["JSON".to_string()]]);
    }
}
