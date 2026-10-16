use super::{append_reply, push_tool_failure, push_tool_note, replay_messages};
use crate::{
    agent::AgentEvent,
    session::SessionSummary,
    tools,
    tui::{Activity, App, Picker, PickerKind, Role, time_ago, worker::UiEvent},
};
use anyhow::Result;

pub(in crate::tui) fn handle_agent_event(event: UiEvent, app: &mut App) {
    match event {
        UiEvent::Agent(event) => apply_agent_event(event, app),
        UiEvent::McpRestarting {
            name,
            scope,
            notice,
        } => {
            app.push(Role::Event, notice);
            app.start_mcp_servers([(name.as_str(), scope)], []);
        }
        UiEvent::McpServer(mut added) => {
            let diagnostics = std::mem::take(&mut added.diagnostics);
            app.finish_mcp_server(added);
            for diagnostic in diagnostics {
                app.push(Role::Event, diagnostic);
            }
        }
        UiEvent::Update(progress) => app.show_update(&progress),
        UiEvent::Done(result) => finish_reply(app, result),
        UiEvent::Cancelled(result) => {
            app.tasks.clear();
            app.push(Role::Event, "cancelled");
            app.restore_queued();
            app.finish(result, |_, ()| {});
        }
        UiEvent::Sessions(result) => app.finish(result, open_session_picker),
        UiEvent::NewSession(result) => app.finish(result, |app, ()| {
            app.clear_session();
            app.push(Role::Event, "new session");
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
        UiEvent::Folder(generation, folder, names) => app.set_folder(generation, folder, names),
        UiEvent::Workspace(label) => app.workspace = label,
        UiEvent::Background(message) => {
            app.start_background();
            app.push_background(&message);
        }
        UiEvent::Resumed(result) => app.finish(result, |app, messages| {
            app.clear_session();
            replay_messages(app, &messages);
            app.push(Role::Event, "resumed session");
        }),
    }
}

fn apply_agent_event(event: AgentEvent, app: &mut App) {
    match event {
        AgentEvent::Text(text) => append_reply(app, &text),
        AgentEvent::Thinking(text) => match app.messages.last_mut() {
            Some(last) if matches!(last.role, Role::Thinking) => last.append(&text),
            _ => app.push(Role::Thinking, text),
        },
        AgentEvent::ToolStart {
            name,
            summary,
            diff,
        } => {
            if name == tools::agent::NAME {
                app.tasks.clear();
            }
            app.push_tool(&name, summary, diff.as_deref());
        }
        AgentEvent::ToolDone { name, error, note } => {
            if name == "bash" {
                app.workspace_stale = true;
            }
            if let Some(error) = error {
                push_tool_failure(app, &name, &error);
            }
            if let Some(note) = note {
                push_tool_note(app, &name, &note);
            }
        }
        AgentEvent::Notice(text) => app.push(Role::Event, text),
        AgentEvent::TaskStart { task, description } => {
            app.start_task(task, &description);
        }
        AgentEvent::TaskToolCalls { task, count } => {
            app.count_task_tool_calls(task, count);
        }
        AgentEvent::TaskStats { task, stats } => app.show_task_stats(task, stats),
        AgentEvent::TaskDone {
            task,
            reply,
            tool_calls,
        } => {
            app.finish_task(task, tool_calls, reply.err().as_deref());
        }
        AgentEvent::Queued(prompt) => app.push(Role::User, prompt),
        AgentEvent::Stats(stats) => app.stats = stats,
    }
}

fn finish_reply(app: &mut App, result: Result<()>) {
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

fn open_session_picker(app: &mut App, sessions: Vec<SessionSummary>) {
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
}
