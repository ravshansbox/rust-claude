use std::borrow::Cow;

use anyhow::{Context, Result};

use super::{Scope, Skill, split_frontmatter};

const COMMAND_PREFIX: &str = "/skill:";

pub(crate) fn escape_xml(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    push_escaped_xml(&mut escaped, text);
    escaped
}

fn push_escaped_xml(escaped: &mut String, text: &str) {
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
        prompt.push_str("  <skill>\n    <name>");
        push_escaped_xml(&mut prompt, &skill.name);
        prompt.push_str("</name>\n    <description>");
        push_escaped_xml(&mut prompt, &skill.description);
        prompt.push_str("</description>\n    <location>");
        push_escaped_xml(&mut prompt, &skill.path.to_string_lossy());
        prompt.push_str("</location>\n  </skill>\n");
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
mod tests {
    use super::*;
    use crate::tools::test_support::TemporaryDir;
    use std::path::Path;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn skill(name: &str, path: &Path, disable_model_invocation: bool) -> Skill {
        Skill {
            name: name.into(),
            description: "Use <this> & that.".into(),
            path: path.join("SKILL.md"),
            base_dir: path.to_path_buf(),
            disable_model_invocation,
            scope: Scope::Global,
        }
    }

    #[test]
    fn formats_visible_skills_for_prompt() {
        let skills = [
            skill("shown", Path::new("/skills/shown"), false),
            skill("hidden", Path::new("/skills/hidden"), true),
        ];
        let prompt = format_for_prompt(&skills).unwrap();
        assert!(prompt.contains(
            "  <skill>\n    <name>shown</name>\n    <description>Use &lt;this&gt; &amp; that.</description>\n    <location>/skills/shown/SKILL.md</location>\n  </skill>"
        ));
        assert!(!prompt.contains("hidden"));
        assert_eq!(format_for_prompt(&skills[1..]), None);
    }

    #[test]
    fn lists_global_skills_before_project_ones_in_prompt() {
        let mut project = skill("project", Path::new("/skills/project"), false);
        project.scope = Scope::Project;
        let global = skill("global", Path::new("/skills/global"), false);
        let prompt = format_for_prompt(&[project, global]).unwrap();
        assert!(prompt.find("<name>global").unwrap() < prompt.find("<name>project").unwrap());
    }

    #[test]
    fn expands_and_parses_skill_commands() {
        let directory = TemporaryDir::new("skills-expand");
        let root = directory.path();
        let dir = root.join("demo");
        write(
            &dir.join("SKILL.md"),
            "---\ndescription: Demo.\n---\n\n# Demo\n\nDo it.\n",
        );
        let skills = [skill("demo", &dir, true)];
        let expanded = expand_command("/skill:demo  fix the bug ", &skills).unwrap();
        assert_eq!(
            expanded,
            format!(
                "<skill name=\"demo\" location=\"{}\">\nReferences are relative to {}.\n\n# Demo\n\nDo it.\n</skill>\n\nfix the bug",
                dir.join("SKILL.md").display(),
                dir.display()
            )
        );
        assert_eq!(
            parse_block(&expanded),
            Some(SkillBlock {
                name: "demo".into(),
                user_message: Some("fix the bug"),
            })
        );
        let bare = expand_command("/skill:demo", &skills).unwrap();
        assert_eq!(
            parse_block(&bare),
            Some(SkillBlock {
                name: "demo".into(),
                user_message: None,
            })
        );
        for separated in ["/skill:demo\nfix the bug", "/skill:demo\tfix the bug"] {
            let expanded = expand_command(separated, &skills).unwrap();
            assert_eq!(
                parse_block(&expanded),
                Some(SkillBlock {
                    name: "demo".into(),
                    user_message: Some("fix the bug"),
                }),
                "{separated:?}"
            );
        }
        assert_eq!(
            expand_command("/skill:other x", &skills).unwrap(),
            "/skill:other x"
        );
        assert_eq!(parse_block("plain text"), None);
    }

    #[test]
    fn parses_skill_blocks_whose_body_shows_a_skill_block() {
        let directory = TemporaryDir::new("skills-expand-nested");
        let root = directory.path();
        let dir = root.join("demo");
        write(
            &dir.join("SKILL.md"),
            "---\ndescription: Demo.\n---\nA call looks like:\n\n<skill name=\"x\" location=\"y\">\nBody\n</skill>\n\nrequest\n\nDo it.\n",
        );
        let skills = [skill("demo", &dir, true)];
        let expanded = expand_command("/skill:demo fix the bug", &skills).unwrap();
        assert_eq!(
            parse_block(&expanded),
            Some(SkillBlock {
                name: "demo".into(),
                user_message: Some("fix the bug"),
            })
        );
        let bare = expand_command("/skill:demo", &skills).unwrap();
        assert_eq!(
            parse_block(&bare),
            Some(SkillBlock {
                name: "demo".into(),
                user_message: None,
            })
        );
    }

    #[test]
    fn parses_skill_blocks_with_quotes_in_name_and_path() {
        let directory = TemporaryDir::new("skills-expand-quotes");
        let root = directory.path();
        let dir = root.join("say \"hi\" & <go>");
        write(
            &dir.join("SKILL.md"),
            "---\ndescription: Demo.\n---\nDo it.\n",
        );
        let skills = [skill("it's\"x\"", &dir, true)];
        let expanded = expand_command("/skill:it's\"x\" now", &skills).unwrap();
        assert_eq!(
            parse_block(&expanded),
            Some(SkillBlock {
                name: "it's\"x\"".into(),
                user_message: Some("now"),
            })
        );
    }
}
