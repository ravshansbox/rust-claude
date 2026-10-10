use super::{
    Activity, App, Picker, PickerKind, Role, display_model, status::format_context_use, time_ago,
    worker::UiEvent,
};
use crate::{
    agent::{AgentEvent, parse_shell_message},
    models, skills, tools,
};
use serde_json::Value;

pub(super) fn handle_agent_event(event: UiEvent, app: &mut App) {
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
        UiEvent::McpRestarting {
            name,
            label,
            notice,
        } => {
            app.push(Role::Event, notice);
            app.start_mcp_server(&name, &label);
        }
        UiEvent::McpServer(added) => {
            app.finish_mcp_server(&added.name, added.status);
            for diagnostic in added.diagnostics {
                app.push(Role::Event, diagnostic);
            }
        }
        UiEvent::Update(progress) => app.push(Role::Event, progress.message()),
        UiEvent::Agent(AgentEvent::Queued(prompt)) => app.push(Role::User, prompt),
        UiEvent::Agent(AgentEvent::Stats(stats)) => app.stats = stats,
        UiEvent::Done(result) => {
            // Esc may come just as the reply finishes; it still keeps the
            // queued prompts from being sent.
            let cancelled = app.activity == Some(Activity::Cancelling);
            if cancelled && result.is_ok() {
                app.push(Role::Event, "cancelled");
            }
            if cancelled || result.is_err() {
                app.restore_queued();
            }
            app.finish(result, |_, ()| {});
        }
        UiEvent::Cancelled(result) => {
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
        UiEvent::Context(context) => {
            app.push(Role::Event, format_context_use(&context));
            app.activity = None;
        }
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
        test_support::{new_app, screen},
    };
    use crossterm::event::{Event, KeyCode, KeyEvent};
    use serde_json::json;

    #[test]
    fn shows_queued_prompt_when_the_agent_adds_it() {
        let mut app = new_app();
        handle_agent_event(UiEvent::Agent(AgentEvent::Queued("next".into())), &mut app);
        let last = app.messages.last().unwrap();
        assert!(matches!(last.role, Role::User));
        assert_eq!(last.text, "next");
    }

    #[test]
    fn shows_update_progress() {
        let mut app = new_app();
        for progress in [
            crate::update::Progress::Available("v0.2.0".into()),
            crate::update::Progress::Ready("v0.2.0".into()),
        ] {
            handle_agent_event(UiEvent::Update(progress), &mut app);
        }
        let shown = screen(&mut app);
        let available = shown.find("update v0.2.0 available").expect(&shown);
        let ready = shown
            .find("installed v0.2.0, restart rust-claude to use it")
            .expect(&shown);
        assert!(available < ready, "{shown}");
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
    fn updates_branch_in_status_line_after_commands() {
        let finished_tool = || {
            UiEvent::Agent(AgentEvent::ToolDone {
                name: "bash".into(),
                error: None,
                note: None,
            })
        };
        let finished_shell = || UiEvent::Shell("git switch other".into(), Ok(String::new()));
        for event in [finished_tool(), finished_shell()] {
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
}
