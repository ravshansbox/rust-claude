use crate::skills::Skill;

pub(super) const COMMANDS: &[(&str, &str)] = &[
    ("/new", "start a new session"),
    ("/compact", "summarise the conversation to free context"),
    ("/context", "show what fills the context"),
    ("/mcp", "sign in to an MCP server"),
    ("/resume", "resume a previous session"),
    ("/model", "select model"),
    ("/thinking", "select thinking level"),
    ("/quit", "quit"),
];

pub(super) const MCP_SUBCOMMANDS: &[(&str, &str)] = &[("login", "sign in to an MCP server")];

/// `/mcp` subcommands, or the servers they apply to, starting with the
/// input after `/mcp `.
fn mcp_matches(rest: &str, sign_in_servers: &[String]) -> Vec<(String, String)> {
    match rest.split_once(' ') {
        Some((subcommand, server)) => {
            let known = MCP_SUBCOMMANDS.iter().any(|(name, _)| *name == subcommand);
            if !known || server.contains(char::is_whitespace) {
                return Vec::new();
            }
            sign_in_servers
                .iter()
                .filter(|name| name.starts_with(server))
                .map(|name| (format!("/mcp {subcommand} {name}"), String::new()))
                .collect()
        }
        None => MCP_SUBCOMMANDS
            .iter()
            .filter(|(name, _)| name.starts_with(rest))
            .map(|(name, description)| (format!("/mcp {name}"), description.to_string()))
            .collect(),
    }
}

/// Commands and skill commands starting with the input, with the one named
/// exactly by the input first, so Enter runs it rather than a longer name.
pub(super) fn command_matches(
    input: &str,
    skills: &[Skill],
    sign_in_servers: &[String],
) -> Vec<(String, String)> {
    if let Some(rest) = input.strip_prefix("/mcp ") {
        let mut matches = mcp_matches(rest, sign_in_servers);
        matches.sort();
        return matches;
    }
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return Vec::new();
    }
    let commands = COMMANDS
        .iter()
        .map(|(name, description)| (name.to_string(), description.to_string()));
    let skill_commands = skills.iter().map(|skill| {
        (
            format!("/skill:{}", skill.name),
            format!(
                "[{}] {}",
                skill.scope,
                skill
                    .description
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        )
    });
    let mut matches: Vec<(String, String)> = commands
        .chain(skill_commands)
        .filter(|(name, _)| name.starts_with(input))
        .collect();
    matches.sort();
    if let Some(exact) = matches.iter().position(|(name, _)| name == input) {
        matches[..=exact].rotate_right(1);
    }
    matches
}
