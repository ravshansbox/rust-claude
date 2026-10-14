use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde_json::{Value, json};

use crate::config;

mod prompt;
pub(crate) use prompt::escape_xml;
pub use prompt::{expand_command, format_for_prompt, parse_block, parse_command};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];
const SKILL_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub base_dir: PathBuf,
    pub disable_model_invocation: bool,
    pub scope: Scope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project,
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(match self {
            Self::Global => "global",
            Self::Project => "project",
        })
    }
}

#[derive(Debug, Default)]
pub struct Skills {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    RustClaude,
    Agents,
}

pub fn load() -> Skills {
    let config_dir = config::dir();
    let home = config::home();
    let cwd = std::env::current_dir().unwrap_or_default();
    load_from(config_dir.as_deref(), home.as_deref(), &cwd)
}

/// Whether two paths name the same folder, so a project `.rust-coding-agent` run
/// from the home folder is not mistaken for a second, project-level one.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

fn load_from(config_dir: Option<&Path>, home: Option<&Path>, cwd: &Path) -> Skills {
    let project_dir = cwd.join(".rust-coding-agent");
    let user_agents_dir = home.map(agents_skill_dir);
    let project = (!config_dir.is_some_and(|config_dir| same_dir(config_dir, &project_dir)))
        .then(|| (project_dir.join("skills"), Mode::RustClaude, Scope::Project));
    let ancestors = ancestor_agents_skill_dirs(cwd)
        .into_iter()
        .filter(|dir| {
            !user_agents_dir
                .as_ref()
                .is_some_and(|user| same_dir(user, dir))
        })
        .map(|dir| (dir, Mode::Agents, Scope::Project));
    let global = config_dir.map(|dir| (dir.join("skills"), Mode::RustClaude, Scope::Global));
    let user = user_agents_dir
        .clone()
        .map(|dir| (dir, Mode::Agents, Scope::Global));
    let paths = project
        .into_iter()
        .chain(ancestors)
        .chain(global)
        .chain(user)
        .flat_map(|(dir, mode, scope)| {
            collect_skill_files(&dir, mode)
                .into_iter()
                .map(move |path| (path, scope))
        });

    let mut skills: Vec<Skill> = Vec::new();
    let mut real_paths = HashSet::new();
    let mut diagnostics = Vec::new();
    let mut collisions = Vec::new();
    for (path, scope) in paths {
        let (skill, warnings) = load_skill_file(&path, scope);
        diagnostics.extend(warnings);
        let Some(skill) = skill else {
            continue;
        };
        let real_path = skill
            .path
            .canonicalize()
            .unwrap_or_else(|_| skill.path.clone());
        if real_paths.contains(&real_path) {
            continue;
        }
        if let Some(existing) = skills.iter().find(|existing| existing.name == skill.name) {
            collisions.push(format!(
                "skill \"{}\" collision: kept {}, skipped {}",
                skill.name,
                existing.path.display(),
                skill.path.display()
            ));
        } else {
            real_paths.insert(real_path);
            skills.push(skill);
        }
    }
    diagnostics.extend(collisions);
    Skills {
        skills,
        diagnostics,
    }
}

fn ancestor_agents_skill_dirs(start: &Path) -> Vec<PathBuf> {
    let Some(repository_root) = start.ancestors().find(|dir| dir.join(".git").exists()) else {
        return vec![agents_skill_dir(start)];
    };
    start
        .ancestors()
        .take_while(|dir| *dir != repository_root)
        .chain([repository_root])
        .map(agents_skill_dir)
        .collect()
}

fn agents_skill_dir(dir: &Path) -> PathBuf {
    dir.join(".agents").join("skills")
}

fn collect_skill_files(dir: &Path, mode: Mode) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_from(
        dir,
        dir,
        mode,
        &mut Vec::new(),
        &mut HashSet::new(),
        &mut files,
    );
    files
}

fn ignore_rules(dir: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(dir);
    for file_name in IGNORE_FILE_NAMES {
        let path = dir.join(file_name);
        if path.is_file() {
            let _ = builder.add(path);
        }
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

/// Asks the deepest directory's ignore files first, like git does.
fn is_ignored(rules: &[Gitignore], path: &Path, is_dir: bool) -> bool {
    rules
        .iter()
        .rev()
        .map(|rules| rules.matched(path, is_dir))
        .find(|matched| !matched.is_none())
        .is_some_and(|matched| matched.is_ignore())
}

fn collect_from(
    dir: &Path,
    root: &Path,
    mode: Mode,
    rules: &mut Vec<Gitignore>,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let Ok(real_dir) = dir.canonicalize() else {
        return;
    };
    if !visited.insert(real_dir) {
        return;
    }
    rules.push(ignore_rules(dir));
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);

    let skill_file = dir.join(SKILL_FILE);
    if skill_file.is_file() && !is_ignored(rules, &skill_file, false) {
        files.push(skill_file);
        rules.pop();
        return;
    }

    for entry in entries {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        let include_markdown = metadata.is_file()
            && name.ends_with(".md")
            && !is_ignored(rules, &path, false)
            && match mode {
                Mode::RustClaude => dir == root,
                Mode::Agents => dir != root,
            };
        if include_markdown {
            files.push(path);
            continue;
        }
        if !metadata.is_dir() || is_ignored(rules, &path, true) {
            continue;
        }
        collect_from(&path, root, mode, rules, visited, files);
    }
    rules.pop();
}

fn split_frontmatter(content: &str) -> (Option<String>, String) {
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

fn load_skill_file(path: &Path, scope: Scope) -> (Option<Skill>, Vec<String>) {
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

#[cfg(test)]
mod tests;
