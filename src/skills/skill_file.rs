use std::borrow::Cow;
use std::path::Path;

use anyhow::Result;
use serde_json::{Value, json};

use super::{SKILL_FILE, Scope, Skill};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

pub(super) fn split_frontmatter(content: &str) -> (Option<String>, String) {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let normalised = if content.contains('\r') {
        Cow::Owned(content.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(content)
    };
    if !normalised.starts_with("---") {
        return (None, normalised.into_owned());
    }
    let Some(end) = normalised[3..].find("\n---").map(|index| index + 3) else {
        return (None, normalised.into_owned());
    };
    let yaml = normalised
        .get(4..end)
        .filter(|yaml| !yaml.is_empty())
        .map(str::to_string);
    let body = normalised[end + 4..].trim().to_string();
    (yaml, body)
}

fn parse_frontmatter(content: &str) -> Result<Value> {
    let value: Option<Value> = split_frontmatter(content)
        .0
        .map(|yaml| yaml_serde::from_str(&yaml))
        .transpose()?;
    Ok(value
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| json!({})))
}

fn validate_name(name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let length = name.chars().count();
    if length > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {MAX_NAME_LENGTH} characters ({length})"
        ));
    }
    if name.is_empty()
        || !name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)".into(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".into());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".into());
    }
    errors
}

fn validate_description(description: Option<&str>) -> Vec<String> {
    let Some(description) = description else {
        return vec!["description is required".into()];
    };
    let length = description.chars().count();
    if length > MAX_DESCRIPTION_LENGTH {
        vec![format!(
            "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({length})"
        )]
    } else {
        Vec::new()
    }
}

pub(super) fn load_skill_file(path: &Path, scope: Scope) -> (Option<Skill>, Vec<String>) {
    let warning = |message: String| format!("skill warning: {}: {message}", path.display());
    let is_declared_skill = path.file_name().is_some_and(|name| name == SKILL_FILE);
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => return (None, vec![warning(error.to_string())]),
    };
    let frontmatter = match parse_frontmatter(&content) {
        Ok(frontmatter) => frontmatter,
        Err(error) if is_declared_skill => return (None, vec![warning(error.to_string())]),
        Err(_) => return (None, Vec::new()),
    };
    let description = frontmatter["description"]
        .as_str()
        .filter(|description| !description.trim().is_empty());
    if !is_declared_skill && description.is_none() {
        return (None, Vec::new());
    }
    let base_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut diagnostics: Vec<String> = validate_description(description)
        .into_iter()
        .map(&warning)
        .collect();
    let name = frontmatter["name"]
        .as_str()
        .filter(|name| !name.is_empty())
        .map_or_else(
            || {
                let fallback = if is_declared_skill {
                    base_dir.file_name()
                } else {
                    path.file_stem()
                };
                fallback
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            },
            str::to_string,
        );
    diagnostics.extend(validate_name(&name).into_iter().map(&warning));
    let Some(description) = description else {
        return (None, diagnostics);
    };
    let skill = Skill {
        name,
        description: description.to_string(),
        path: path.to_path_buf(),
        base_dir,
        disable_model_invocation: frontmatter["disable-model-invocation"] == true,
        scope,
    };
    (Some(skill), diagnostics)
}
