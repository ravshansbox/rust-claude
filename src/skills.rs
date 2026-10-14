use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::config;

mod prompt;
mod skill_file;
pub(crate) use prompt::escape_xml;
pub use prompt::{expand_command, format_for_prompt, parse_block, parse_command};
use skill_file::{load_skill_file, split_frontmatter};

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
    let skill_file = dir.join(SKILL_FILE);
    if skill_file.is_file() && !is_ignored(rules, &skill_file, false) {
        files.push(skill_file);
        rules.pop();
        return;
    }

    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
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

#[cfg(test)]
mod tests;
