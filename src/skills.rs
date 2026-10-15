use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::config;

mod prompt;
mod skill_file;
mod walk;
pub(crate) use prompt::escape_xml;
pub use prompt::{expand_command, format_for_prompt, parse_block, parse_command};
use skill_file::{load_skill_file, split_frontmatter};
use walk::{Mode, collect_skill_files};

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

#[cfg(test)]
mod tests;
