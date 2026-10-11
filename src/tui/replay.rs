use super::{Activity, App, Picker, PickerKind, Role, display_model, time_ago, worker::UiEvent};
use crate::{
    agent::{AgentEvent, parse_shell_message},
    models, skills, tools,
};
use serde_json::Value;

pub(super) fn handle_agent_event(event: UiEvent, app: &mut App) {
    match event {
        UiEvent::Agent(AgentEvent::Text(text)) => append_reply(app, &text),
        UiEvent::Agent(AgentEvent::Thinking(text)) => match app.messages.last_mut() {
            Some(last) if matches!(last.role, Role::Thinking) => last.append(&text),
            _ => app.push(Role::Thinking, text),
        },
        UiEvent::Agent(AgentEvent::ToolStart {
            name,
            summary,
            diff,
        }) => {
            if name == tools::agent::NAME {
                app.tasks.clear();
            }
            app.push_tool(&name, summary, diff);
        }
        UiEvent::Agent(AgentEvent::ToolDone { name, error, note }) => {
            if name == "bash" {
                app.workspace_stale = true;
            }
            if let Some(error) = error {
                app.push(Role::Event, format!("{name} failed: {error}"));
            }
            if let Some(note) = note {
                app.push(Role::Event, format!("{name}: {note}"));
            }
        }
        UiEvent::Agent(AgentEvent::Notice(text)) => app.push(Role::Event, text),
        UiEvent::Agent(AgentEvent::TaskStart { task, description }) => {
            app.start_task(task, &description);
        }
        UiEvent::Agent(AgentEvent::TaskToolCalls { task, count }) => {
            app.count_task_tool_calls(task, count);
        }
        UiEvent::Agent(AgentEvent::TaskDone {
            task,
            reply,
            tool_calls,
        }) => {
            app.finish_task(task, tool_calls, reply.err().as_deref());
        }
        UiEvent::McpRestarting {
            name,
            scope,
            notice,
        } => {
            app.push(Role::Event, notice);
            app.start_mcp_servers([(name.as_str(), scope)]);
        }
        UiEvent::McpServer(mut added) => {
            let diagnostics = std::mem::take(&mut added.diagnostics);
            app.finish_mcp_server(added);
            for diagnostic in diagnostics {
                app.push(Role::Event, diagnostic);
            }
        }
        UiEvent::Update(progress) => app.show_update(&progress),
        UiEvent::WhatsNew(notes) => app.push(Role::Event, notes),
        UiEvent::Agent(AgentEvent::Queued(prompt)) => app.push(Role::User, prompt),
        UiEvent::Agent(AgentEvent::Stats(stats)) => app.stats = stats,
        UiEvent::Done(result) => {
            app.tasks.clear();
            // Esc may come just as the reply finishes; it still keeps the
            // queued prompts from being sent.
            let cancelled = app.activity == Some(Activity::Cancelling);
            if cancelled && result.is_ok() {
                app.push(Role::Event, "cancelled");
            }
            if cancelled || result.is_err() {
                app.restore_queued();
            }
            if app.notifications && !app.focused && !cancelled {
                app.notification = Some(match &result {
                    Ok(()) => "Done".into(),
                    Err(error) => {
                        let error = format!("{error:#}");
                        format!("Failed: {}", error.lines().next().unwrap_or_default())
                    }
                });
            }
            app.finish(result, |_, ()| {});
        }
        UiEvent::Cancelled(result) => {
            app.tasks.clear();
            app.push(Role::Event, "cancelled");
            app.restore_queued();
            app.finish(result, |_, ()| {});
        }
        UiEvent::Sessions(result) => app.finish(result, |app, sessions| {
            if sessions.is_empty() {
                app.push(Role::Event, "no session to resume");
                return;
            }
            app.open_picker(Picker {
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
            let models = models::latest_in_each_class(models);
            if models.is_empty() {
                app.push(Role::Event, "no models available");
                return;
            }
            let selected = models
                .iter()
                .position(|model| *model == app.model)
                .unwrap_or_default();
            app.open_picker(Picker {
                kind: PickerKind::Model,
                title: "Select model",
                items: models
                    .into_iter()
                    .map(|model| (model.clone(), display_model(&model).to_string()))
                    .collect(),
                selected,
            });
        }),
        UiEvent::Shell(command, result) => {
            app.workspace_stale = true;
            app.finish(result, |app, output| app.push_shell(&command, &output));
        }
        UiEvent::ImagePasted(Ok(Some(image))) => app.attach_image(image),
        UiEvent::ImagePasted(Ok(None)) => app.push(Role::Event, "no image in the clipboard"),
        UiEvent::ImagePasted(Err(error)) => {
            app.push(Role::Event, format!("failed to paste image: {error:#}"));
        }
        UiEvent::Files(generation, files) => app.set_files(generation, files),
        UiEvent::Workspace(label) => app.workspace = label,
        UiEvent::Resumed(result) => app.finish(result, |app, messages| {
            app.clear_session();
            replay_messages(app, &messages);
            app.push(Role::Event, "resumed session");
        }),
    }
}

pub(super) fn replay_messages(app: &mut App, messages: &[Value]) {
    let results: std::collections::HashMap<&str, (&Value, bool)> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| {
            Some((
                block["tool_use_id"].as_str()?,
                (&block["content"], block["is_error"] == true),
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
                    append_reply(app, block["text"].as_str().unwrap_or_default());
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
                            let error = error.as_str().unwrap_or_default();
                            app.push(Role::Event, format!("{name} failed: {error}"));
                        }
                        Some((Value::Array(replies), false)) if name == tools::agent::NAME => {
                            let descriptions = block["input"]["tasks"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|task| task["description"].as_str().unwrap_or_default());
                            let replies = replies
                                .iter()
                                .map(|reply| reply["text"].as_str().unwrap_or_default());
                            app.push_finished_tasks(descriptions.zip(replies));
                        }
                        Some((result, false)) => {
                            if let Some(note) =
                                tools::note(name, result.as_str().unwrap_or_default())
                            {
                                app.push(Role::Event, format!("{name}: {note}"));
                            }
                        }
                        None => {}
                    }
                }
                ("assistant", "server_tool_use") => {
                    let name = block["name"].as_str().unwrap_or_default();
                    app.push_tool(name, tools::summary(name, &block["input"]), None);
                }
                ("assistant", "web_search_tool_result") => {
                    let name = tools::web_search::NAME;
                    match tools::web_search::outcome(&block["content"]) {
                        Ok(note) => app.push(Role::Event, format!("{name}: {note}")),
                        Err(error) => app.push(Role::Event, format!("{name} failed: {error}")),
                    }
                }
                ("user", "text") => replay_prompt(app, block["text"].as_str().unwrap_or_default()),
                _ => {}
            }
        }
        if role == "assistant"
            && let Some(blocks) = message["content"].as_array()
            && let Some(sources) = tools::web_search::sources(blocks)
        {
            append_reply(app, &sources);
        }
    }
}

fn append_reply(app: &mut App, text: &str) {
    if !app
        .messages
        .last()
        .is_some_and(|last| matches!(last.role, Role::Assistant))
    {
        app.push(Role::Assistant, "");
    }
    if let Some(last) = app.messages.last_mut() {
        last.append(text);
    }
}

fn replay_prompt(app: &mut App, text: &str) {
    if let Some((command, output)) = parse_shell_message(text) {
        app.push_shell(command, output);
        app.add_prompt(format!("!{command}"));
        return;
    }
    match skills::parse_block(text) {
        Some(block) => {
            app.push(Role::Event, format!("[skill] {}", block.name));
            let mut command = format!("/skill:{}", block.name);
            if let Some(user_message) = block.user_message {
                app.push(Role::User, user_message);
                command = format!("{command} {user_message}");
            }
            app.add_prompt(command);
        }
        None => {
            app.push(Role::User, text);
            app.add_prompt(text.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{handle_agent_event, replay_messages};
    use crate::agent::AgentEvent;
    use crate::skills::{Scope, Skill};
    use crate::tools;
    use crate::tui::{
        Action, App, Role, UiEvent, handle_input,
        test_support::{new_app, screen, screen_of_width},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent};
    use serde_json::json;

    #[test]
    fn notifies_when_a_run_ends_while_the_terminal_is_unfocused() {
        let mut app = new_app();
        handle_input(Event::FocusLost, &mut app, |_| {});
        handle_agent_event(UiEvent::Done(Ok(())), &mut app);
        assert_eq!(app.notification.take().as_deref(), Some("Done"));
        handle_agent_event(
            UiEvent::Done(Err(anyhow::anyhow!("overloaded\nretry later"))),
            &mut app,
        );
        assert_eq!(
            app.notification.take().as_deref(),
            Some("Failed: overloaded")
        );
    }

    #[test]
    fn does_not_notify_when_focused_cancelled_or_turned_off() {
        let mut focused = new_app();
        handle_input(Event::FocusLost, &mut focused, |_| {});
        handle_input(Event::FocusGained, &mut focused, |_| {});
        let mut cancelled = new_app();
        handle_input(Event::FocusLost, &mut cancelled, |_| {});
        cancelled.start(crate::tui::Activity::Cancelling);
        let mut off = new_app();
        off.notifications = false;
        handle_input(Event::FocusLost, &mut off, |_| {});
        for mut app in [focused, cancelled, off] {
            handle_agent_event(UiEvent::Done(Ok(())), &mut app);
            assert_eq!(app.notification, None);
        }
    }

    #[test]
    fn shows_queued_prompt_when_the_agent_adds_it() {
        let mut app = new_app();
        handle_agent_event(UiEvent::Agent(AgentEvent::Queued("next".into())), &mut app);
        let last = app.messages.last().unwrap();
        assert!(matches!(last.role, Role::User));
        assert_eq!(last.text, "next");
    }

    #[test]
    fn adds_each_update_step_to_the_version_line() {
        let mut app = new_app();
        app.push(Role::Event, "loaded AGENTS.md");
        let seconds = std::time::Duration::from_secs;
        for progress in [
            crate::update::Progress::Available("v0.2.0".into()),
            crate::update::Progress::Downloading("v0.2.0".into()),
            crate::update::Progress::Downloaded("v0.2.0".into(), seconds(3)),
            crate::update::Progress::Building("v0.2.0".into()),
            crate::update::Progress::Built("v0.2.0".into(), seconds(125)),
            crate::update::Progress::Ready("v0.2.0".into()),
        ] {
            handle_agent_event(UiEvent::Update(progress), &mut app);
        }
        let shown = screen_of_width(&mut app, 160);
        let version = shown
            .find(concat!(
                "rust-coding-agent v",
                env!("CARGO_PKG_VERSION"),
                " · update v0.2.0 available · downloaded in 3s · built in 2m 5s",
                " · installed, restart rust-coding-agent to use it"
            ))
            .expect(&shown);
        assert!(
            version < shown.find("loaded AGENTS.md").expect(&shown),
            "{shown}"
        );
    }

    #[test]
    fn shows_whats_new_after_an_update() {
        let mut app = new_app();
        handle_agent_event(
            UiEvent::WhatsNew("what's new in v0.2.0:\n- add search".into()),
            &mut app,
        );
        let shown = screen(&mut app);
        let notes = shown.find("what's new in v0.2.0:").expect(&shown);
        assert!(notes < shown.find("- add search").expect(&shown), "{shown}");
    }

    #[test]
    fn replaces_the_failed_step_with_why_it_failed() {
        let mut app = new_app();
        let seconds = std::time::Duration::from_secs;
        for progress in [
            crate::update::Progress::Available("v0.2.0".into()),
            crate::update::Progress::Downloading("v0.2.0".into()),
            crate::update::Progress::Downloaded("v0.2.0".into(), seconds(3)),
            crate::update::Progress::Building("v0.2.0".into()),
            crate::update::Progress::BuildFailed("v0.2.0".into(), "error: x".into(), seconds(70)),
        ] {
            handle_agent_event(UiEvent::Update(progress), &mut app);
        }
        let shown = screen_of_width(&mut app, 160);
        assert!(
            shown.contains(concat!(
                "rust-coding-agent v",
                env!("CARGO_PKG_VERSION"),
                " · update v0.2.0 available · downloaded in 3s",
                " · build failed after 1m 10s: error: x"
            )),
            "{shown}"
        );
    }

    #[test]
    fn shows_update_progress_with_the_version_after_clearing() {
        let mut app = new_app();
        handle_agent_event(
            UiEvent::Update(crate::update::Progress::Available("v0.2.0".into())),
            &mut app,
        );
        app.clear_session();
        handle_agent_event(
            UiEvent::Update(crate::update::Progress::Downloading("v0.2.0".into())),
            &mut app,
        );
        let shown = screen(&mut app);
        assert!(
            shown.contains(concat!(
                "rust-coding-agent v",
                env!("CARGO_PKG_VERSION"),
                " · update v0.2.0 available · downloading"
            )),
            "{shown}"
        );
    }

    #[test]
    fn offers_only_the_latest_model_in_each_class() {
        let mut app = new_app();
        let models = [
            "claude-haiku-4-5-20251001",
            "claude-sonnet-5-5",
            "claude-opus-4-6",
            "claude-haiku-5-5",
            "claude-fable-5",
            "claude-opus-5-5",
            "claude-opus-4-5-20251101",
            "claude-fable-5-1",
            "claude-sonnet-4-5",
        ];
        handle_agent_event(
            UiEvent::Models(Ok(models.map(String::from).to_vec())),
            &mut app,
        );
        let shown = screen(&mut app);
        let rows: Vec<&str> = shown
            .lines()
            .skip_while(|line| !line.starts_with("Select model"))
            .skip(1)
            .take_while(|line| !line.starts_with('─'))
            .map(str::trim)
            .collect();
        assert_eq!(
            rows,
            ["fable-5-1", "opus-5-5", "sonnet-5-5", "haiku-5-5"],
            "{shown}"
        );
    }

    #[test]
    fn replays_shell_commands() {
        let mut app = new_app();
        replay_messages(
            &mut app,
            &[json!({ "role": "user", "content": crate::agent::shell_message("pwd", "/tmp\n") })],
        );
        assert!(matches!(app.messages.last().unwrap().role, Role::Tool));
        assert_eq!(app.messages.last().unwrap().text, "! pwd\n/tmp");
        assert_eq!(app.prompt_history, ["!pwd"]);
    }

    #[test]
    fn replays_failed_tool_calls() {
        let mut app = new_app();
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
        let mut app = new_app();
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
        let mut live = new_app();
        live.skills = vec![Skill {
            name: "demo".into(),
            description: "Run\nthe demo.".into(),
            path: "/skills/demo/SKILL.md".into(),
            base_dir: "/skills/demo".into(),
            disable_model_invocation: false,
            scope: Scope::Global,
        }];
        handle_input(Event::Paste("/sk".into()), &mut live, |_| {});
        assert_eq!(
            live.visible_suggestions().unwrap().items,
            [(
                "/skill:demo".to_string(),
                "[global] Run the demo.".to_string()
            )]
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
        let mut replayed = new_app();
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
        let input =
            json!({ "path": "a.rs", "old_text": "a", "new_text": "b", "replace_all": true });
        let mut live = new_app();
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
        let mut replayed = new_app();
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

    #[test]
    fn shows_web_search_and_sources_live_and_replayed() {
        let input = json!({ "query": "rust release" });
        let mut live = new_app();
        let events = [
            AgentEvent::ToolStart {
                name: "web_search".into(),
                summary: tools::summary("web_search", &input),
                diff: None,
            },
            AgentEvent::ToolDone {
                name: "web_search".into(),
                error: None,
                note: Some("1 result".into()),
            },
            AgentEvent::Text("Rust 1.99.0".into()),
            AgentEvent::Text(" is the latest.".into()),
            AgentEvent::Text("\n\nSources:\n- Rust Versions: https://releases.rs/\n".into()),
        ];
        for event in events {
            handle_agent_event(UiEvent::Agent(event), &mut live);
        }
        let citation = json!({ "type": "web_search_result_location", "url": "https://releases.rs/", "title": "Rust Versions" });
        let mut replayed = new_app();
        replay_messages(
            &mut replayed,
            &[json!({ "role": "assistant", "content": [
                { "type": "server_tool_use", "id": "s1", "name": "web_search", "input": input },
                { "type": "web_search_tool_result", "tool_use_id": "s1", "content": [{ "type": "web_search_result", "url": "https://releases.rs/", "title": "Rust Versions" }] },
                { "type": "text", "text": "Rust 1.99.0", "citations": [citation] },
                { "type": "text", "text": " is the latest." },
            ] })],
        );
        let texts = |app: &App| -> Vec<String> {
            app.messages[1..]
                .iter()
                .map(|message| message.text.clone())
                .collect()
        };
        assert_eq!(
            texts(&live),
            [
                "web_search rust release",
                "web_search: 1 result",
                "Rust 1.99.0 is the latest.\n\nSources:\n- Rust Versions: https://releases.rs/\n"
            ]
        );
        assert_eq!(texts(&replayed), texts(&live));
    }

    #[test]
    fn replays_a_failed_web_search() {
        let mut replayed = new_app();
        replay_messages(
            &mut replayed,
            &[json!({ "role": "assistant", "content": [
                { "type": "server_tool_use", "id": "s1", "name": "web_search", "input": { "query": "q" } },
                { "type": "web_search_tool_result", "tool_use_id": "s1", "content": { "type": "web_search_tool_result_error", "error_code": "too_many_requests" } },
            ] })],
        );
        let texts: Vec<&str> = replayed.messages[1..]
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            ["web_search q", "web_search failed: too_many_requests"]
        );
    }

    #[test]
    fn updates_branch_in_status_line_after_commands() {
        let finished_tool = |name: &str| {
            UiEvent::Agent(AgentEvent::ToolDone {
                name: name.into(),
                error: None,
                note: None,
            })
        };
        let finished_shell = || UiEvent::Shell("git switch other".into(), Ok(String::new()));
        for event in [finished_tool("bash"), finished_shell()] {
            let mut app = new_app();
            app.workspace = "folder · stale-branch".into();
            handle_agent_event(event, &mut app);
            assert!(app.workspace_stale);
            handle_agent_event(UiEvent::Workspace("folder · other".into()), &mut app);
            let screen = screen(&mut app);
            assert!(!screen.contains("stale-branch"));
            assert!(screen.contains("folder · other"));
        }
    }

    #[test]
    fn keeps_status_line_branch_after_file_tools() {
        for name in ["read", "write", "edit"] {
            let mut app = new_app();
            app.workspace = "folder · branch".into();
            handle_agent_event(
                UiEvent::Agent(AgentEvent::ToolDone {
                    name: name.into(),
                    error: None,
                    note: None,
                }),
                &mut app,
            );
            assert!(!app.workspace_stale, "{name}");
            assert!(screen(&mut app).contains("folder · branch"), "{name}");
        }
    }

    #[test]
    fn shows_why_resuming_a_session_failed() {
        let mut app = new_app();
        let error = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            .context("reading /sessions/x.jsonl");
        handle_agent_event(UiEvent::Resumed(Err(error)), &mut app);
        assert_eq!(
            app.messages.last().unwrap().text,
            "error: reading /sessions/x.jsonl: permission denied"
        );
    }

    #[test]
    fn shows_why_pasting_an_image_failed() {
        let mut app = new_app();
        let error = crate::images::prepare(b"not an image".to_vec()).unwrap_err();
        handle_agent_event(UiEvent::ImagePasted(Err(error)), &mut app);
        let text = &app.messages.last().unwrap().text;
        assert!(
            text.starts_with("failed to paste image: unsupported image format: "),
            "{text}"
        );
    }

    fn agent_input() -> serde_json::Value {
        json!({
            "mode": "parallel",
            "tasks": [
                { "description": "say hi", "prompt": "say hi" },
                { "description": "list files", "prompt": "list the files" },
            ],
        })
    }

    fn start_tasks(app: &mut App) {
        let input = agent_input();
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolStart {
                name: "agent".into(),
                summary: tools::summary("agent", &input),
                diff: tools::diff("agent", &input),
            }),
            app,
        );
        for (task, description) in ["say hi", "list files"].into_iter().enumerate() {
            handle_agent_event(
                UiEvent::Agent(AgentEvent::TaskStart {
                    task,
                    description: description.into(),
                }),
                app,
            );
        }
    }

    fn texts(app: &App) -> Vec<String> {
        app.messages[1..]
            .iter()
            .map(|message| message.text.clone())
            .collect()
    }

    #[test]
    fn shows_each_task_while_it_works() {
        let mut app = new_app();
        start_tasks(&mut app);
        assert_eq!(
            texts(&app),
            [
                "agent parallel, 2 tasks",
                "[agent 1] say hi",
                "[agent 2] list files",
            ]
        );
        let screen = screen(&mut app);
        assert!(screen.contains("[agent 1]  say hi"), "{screen}");
    }

    #[test]
    fn shows_how_many_agents_run_on_the_status_line() {
        fn status_line(app: &mut App) -> String {
            screen(app).lines().last().unwrap().trim_end().to_string()
        }
        let mut app = new_app();
        let idle = status_line(&mut app);
        start_tasks(&mut app);
        let mut shown = vec![status_line(&mut app)];
        for task in 0..2 {
            handle_agent_event(
                UiEvent::Agent(AgentEvent::TaskDone {
                    task,
                    reply: Ok("done".into()),
                    tool_calls: 0,
                }),
                &mut app,
            );
            shown.push(status_line(&mut app));
        }
        assert!(idle.ends_with("model:medium"), "{idle}");
        assert_eq!(
            shown,
            [
                format!("{idle} · 2 agents"),
                format!("{idle} · 1 agent"),
                idle.clone(),
            ]
        );
    }

    #[test]
    fn stops_showing_agents_when_the_prompt_ends() {
        for end in [UiEvent::Cancelled(Ok(())), UiEvent::Done(Ok(()))] {
            let mut app = new_app();
            start_tasks(&mut app);
            handle_agent_event(end, &mut app);
            let screen = screen(&mut app);
            assert!(!screen.contains("agents"), "{screen}");
        }
    }

    #[test]
    fn shows_how_many_tool_calls_each_task_made() {
        let mut app = new_app();
        start_tasks(&mut app);
        for (task, count) in [(0, 1), (0, 2), (1, 1)] {
            handle_agent_event(
                UiEvent::Agent(AgentEvent::TaskToolCalls { task, count }),
                &mut app,
            );
        }
        handle_agent_event(
            UiEvent::Agent(AgentEvent::TaskDone {
                task: 1,
                reply: Err("This task failed: 400 Bad Request".into()),
                tool_calls: 1,
            }),
            &mut app,
        );
        assert_eq!(
            texts(&app),
            [
                "agent parallel, 2 tasks",
                "[agent 1] say hi · 2 tool calls",
                "[agent 2] list files · 1 tool call\n  This task failed: 400 Bad Request",
            ]
        );
    }

    #[test]
    fn hides_final_replies_but_shows_why_a_task_failed_live_and_replayed() {
        let reply = "reply 1\nreply 2";
        let failure = "This task failed: 400 Bad Request\nmore detail";
        let mut live = new_app();
        start_tasks(&mut live);
        handle_agent_event(
            UiEvent::Agent(AgentEvent::TaskDone {
                task: 0,
                reply: Ok(reply.into()),
                tool_calls: 0,
            }),
            &mut live,
        );
        handle_agent_event(
            UiEvent::Agent(AgentEvent::TaskDone {
                task: 1,
                reply: Err(failure.into()),
                tool_calls: 0,
            }),
            &mut live,
        );
        handle_agent_event(
            UiEvent::Agent(AgentEvent::ToolDone {
                name: "agent".into(),
                error: None,
                note: None,
            }),
            &mut live,
        );
        let mut replayed = new_app();
        replay_messages(
            &mut replayed,
            &[
                json!({ "role": "assistant", "content": [{ "type": "tool_use", "id": "1", "name": "agent", "input": agent_input() }] }),
                json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "1", "content": [
                    { "type": "text", "text": reply },
                    { "type": "text", "text": failure },
                ], "is_error": false }] }),
            ],
        );
        assert_eq!(
            texts(&live),
            [
                "agent parallel, 2 tasks",
                "[agent 1] say hi",
                "[agent 2] list files\n  This task failed: 400 Bad Request",
            ]
        );
        assert_eq!(texts(&replayed), texts(&live));
    }
}
