use super::{Action, Activity, App, PickerKind, Role};
use crate::{
    models, skills,
    tui::{Picker, display_model},
};

fn effort_picker(app: &App) -> Picker {
    let levels = models::effort_levels(&app.model);
    Picker {
        kind: PickerKind::Effort,
        title: "Select effort level",
        items: levels
            .iter()
            .map(|level| (level.to_string(), level.to_string()))
            .collect(),
        selected: levels
            .iter()
            .position(|level| *level == app.effort)
            .unwrap_or_default(),
    }
}

fn model_picker(current: &str) -> Picker {
    Picker {
        kind: PickerKind::Model,
        title: "Select model",
        items: models::known()
            .map(|model| (model.to_string(), display_model(model).to_string()))
            .collect(),
        selected: models::known()
            .position(|model| model == current)
            .unwrap_or_default(),
    }
}

fn mcp_action(argument: &str) -> Option<Action> {
    match argument
        .split_once(char::is_whitespace)
        .map(|(action, server)| (action, server.trim()))
    {
        Some(("login", server)) if !server.is_empty() => {
            Some(Action::McpSignIn(server.to_string()))
        }
        Some(("logout", server)) if !server.is_empty() => {
            Some(Action::McpSignOut(server.to_string()))
        }
        _ => None,
    }
}

pub(super) fn run_command(prompt: String, app: &mut App, mut act: impl FnMut(Action)) -> bool {
    let trimmed = prompt.trim();
    let (command, argument) = trimmed
        .split_once(char::is_whitespace)
        .map_or((trimmed, ""), |(command, argument)| {
            (command, argument.trim())
        });
    match command {
        "/quit" => return true,
        "/new" => {
            app.start(Activity::StartingNewSession);
            act(Action::NewSession);
        }
        "/compact" => {
            app.start(Activity::Compacting);
            act(Action::Compact(app.effort));
        }
        "/effort" if argument.is_empty() => app.open_picker(effort_picker(app)),
        "/effort" => {
            if !app.set_effort(argument) {
                app.restore_input(prompt);
            }
        }
        "/model" if argument.is_empty() => app.open_picker(model_picker(&app.model)),
        "/model" if models::is_known(argument) => {
            app.set_model(argument);
            act(Action::SetModel(argument.to_string()));
        }
        "/model" => {
            app.push(Role::Event, models::unknown(argument));
            app.restore_input(prompt);
        }
        "/resume" => {
            app.start(Activity::LoadingSessions);
            act(Action::ListSessions);
        }
        "/mcp" => match mcp_action(argument) {
            Some(action) => act(action),
            None => {
                app.push(
                    Role::Event,
                    "usage: /mcp login <server> or /mcp logout <server>",
                );
                app.restore_input(prompt);
            }
        },
        _ if let Some((name, arguments)) = skills::parse_command(&prompt) => {
            if !app.skills.iter().any(|skill| skill.name == name) {
                app.push(Role::Event, format!("unknown skill: {name}"));
                app.restore_input(prompt);
                return false;
            }
            app.push(Role::Event, format!("[skill] {name}"));
            if !arguments.is_empty() {
                app.push(Role::User, arguments);
            }
            act(app.submit(prompt));
        }
        command => {
            app.push(Role::Event, format!("unknown command: {command}"));
            app.restore_input(prompt);
        }
    }
    false
}
