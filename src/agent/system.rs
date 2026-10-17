use std::path::Path;

use serde_json::{Value, json};

use super::Agent;
use crate::{config, skills, tools};

const IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

const SYSTEM_PROMPT: &str =
    "Use your tools to inspect and change files in the current working directory.
Read files before changing them, keep changes focused, run relevant checks, and answer concisely.
Use plain English (ISO 24495-1).";

const INSTRUCTIONS_FILE: &str = "AGENTS.md";

#[derive(Clone)]
pub struct Instructions {
    pub label: String,
    pub(super) text: String,
}

pub(super) fn load_instructions() -> Vec<Instructions> {
    let home = config::home();
    let cwd = std::env::current_dir().unwrap_or_default();
    load_instructions_from(home.as_deref(), &cwd)
}

fn load_instructions_from(home: Option<&Path>, cwd: &Path) -> Vec<Instructions> {
    let mut candidates = Vec::new();
    if let Some(home) = home {
        candidates.push((
            format!("~/{INSTRUCTIONS_FILE}"),
            home.join(INSTRUCTIONS_FILE),
        ));
    }
    if !home.is_some_and(|home| skills::same_dir(home, cwd)) {
        candidates.push((
            format!("./{INSTRUCTIONS_FILE}"),
            cwd.join(INSTRUCTIONS_FILE),
        ));
    }

    candidates
        .into_iter()
        .filter_map(|(label, path)| {
            let text = std::fs::read_to_string(path).ok()?;
            (!text.trim().is_empty()).then_some(Instructions { label, text })
        })
        .collect()
}

impl Agent {
    pub(super) fn system_prompt(&self) -> Vec<Value> {
        let mut system = vec![
            json!({ "type": "text", "text": IDENTITY }),
            json!({ "type": "text", "text": SYSTEM_PROMPT }),
        ];
        if let Some(guidance) = tools::guidance() {
            system.push(json!({ "type": "text", "text": guidance }));
        }
        for instructions in &self.instructions {
            system.push(json!({
                "type": "text",
                "text": format!("# Instructions from {}\n\n{}", instructions.label, instructions.text),
            }));
        }
        if let Some(text) = skills::format_for_prompt(&self.skills.skills) {
            system.push(json!({ "type": "text", "text": text }));
        }
        system
    }
}

#[cfg(test)]
mod tests;
