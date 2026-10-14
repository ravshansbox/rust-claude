use std::{borrow::Cow, fmt::Write};

use anyhow::{Context, Result};

use super::{Scope, Skill, split_frontmatter};

const COMMAND_PREFIX: &str = "/skill:";

pub(crate) fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn unescape_xml(text: &str) -> Cow<'_, str> {
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

pub fn format_for_prompt(skills: &[Skill]) -> Option<String> {
    let mut visible: Vec<&Skill> = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();
    visible.sort_by_key(|skill| skill.scope == Scope::Project);
    if visible.is_empty() {
        return None;
    }
    let mut prompt = String::from(
        "The following skills provide specialized instructions for specific tasks.\n\
         Use the read tool to load a skill's file when the task matches its description.\n\
         When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.\n\
         \n\
         <available_skills>\n",
    );
    for skill in visible {
        let _ = write!(
            prompt,
            "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>\n",
            escape_xml(&skill.name),
            escape_xml(&skill.description),
            escape_xml(&skill.path.to_string_lossy())
        );
    }
    prompt.push_str("</available_skills>");
    Some(prompt)
}

/// Splits `/skill:name request` into the skill name and the trimmed request.
/// The request may follow on the next line.
pub fn parse_command(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix(COMMAND_PREFIX)?;
    Some(
        rest.split_once(char::is_whitespace)
            .map_or((rest, ""), |(name, arguments)| (name, arguments.trim())),
    )
}

pub fn expand_command(text: &str, skills: &[Skill]) -> Result<String> {
    let Some((name, arguments)) = parse_command(text) else {
        return Ok(text.to_string());
    };
    let Some(skill) = skills.iter().find(|skill| skill.name == name) else {
        return Ok(text.to_string());
    };
    let content = std::fs::read_to_string(&skill.path)
        .with_context(|| format!("failed to read skill {}", skill.path.display()))?;
    let body = split_frontmatter(&content).1;
    let block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        escape_xml(&skill.name),
        escape_xml(&skill.path.to_string_lossy()),
        skill.base_dir.display(),
        body.trim()
    );
    Ok(if arguments.is_empty() {
        block
    } else {
        format!("{block}\n\n{arguments}")
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct SkillBlock<'a> {
    pub name: Cow<'a, str>,
    pub user_message: Option<&'a str>,
}

impl SkillBlock<'_> {
    /// The `/skill:name request` command that sent this block.
    pub fn command(&self) -> String {
        match self.user_message {
            Some(user_message) => format!("{COMMAND_PREFIX}{} {user_message}", self.name),
            None => format!("{COMMAND_PREFIX}{}", self.name),
        }
    }
}

pub fn parse_block(text: &str) -> Option<SkillBlock<'_>> {
    let rest = text.strip_prefix("<skill name=\"")?;
    let (name, rest) = rest.split_once('"')?;
    let rest = rest.strip_prefix(" location=\"")?;
    let (location, rest) = rest.split_once('"')?;
    let rest = rest.strip_prefix(">\n")?;
    if name.is_empty() || location.is_empty() {
        return None;
    }
    // The skill body comes from SKILL.md and may itself show a closing tag,
    // so the block ends at the end of the text or at the last closing tag.
    let user_message = if rest.ends_with("\n</skill>") {
        None
    } else {
        let index = rest.rfind("\n</skill>\n\n")?;
        Some(rest[index + "\n</skill>\n\n".len()..].trim())
    };
    Some(SkillBlock {
        name: unescape_xml(name),
        user_message: user_message.filter(|message| !message.is_empty()),
    })
}

#[cfg(test)]
mod tests;
