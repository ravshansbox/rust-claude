use super::{App, Role, replay::replay_messages};
use crate::{
    agent::Agent,
    mcp,
    skills::{Scope, Skills},
    tools,
};
use std::path::PathBuf;

pub(super) fn opening_app(
    agent: &Agent,
    startup: &mcp::Startup,
    history_path: Option<PathBuf>,
) -> App {
    let mut app = App::new(&agent.model, agent.effort, agent.stats());
    app.skills = agent.skills.skills.clone();
    app.mcp_sign_in_servers = startup.sign_in_servers.clone();
    for diagnostic in &startup.diagnostics {
        app.push(Role::Event, diagnostic.clone());
    }
    app.load_history(history_path);
    if !agent.messages().is_empty() {
        replay_messages(&mut app, agent.messages());
        app.push(Role::Event, "continued session");
    }
    app.queue = agent.queue.clone();
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
    app
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

#[cfg(test)]
mod tests;
