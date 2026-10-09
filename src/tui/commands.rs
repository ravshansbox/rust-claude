use crate::skills::Skill;

pub(super) const COMMANDS: &[(&str, &str)] = &[
    ("/new", "start a new session"),
    ("/compact", "summarise the conversation to free context"),
    ("/context", "show what fills the context"),
    ("/resume", "resume a previous session"),
    ("/model", "select model"),
    ("/thinking", "select thinking level"),
    ("/quit", "quit"),
];

pub(super) fn command_matches(input: &str, skills: &[Skill]) -> Vec<(String, String)> {
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
    commands
        .chain(skill_commands)
        .filter(|(name, _)| name.starts_with(input))
        .collect()
}
