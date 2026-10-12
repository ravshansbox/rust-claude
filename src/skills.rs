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

#[derive(Debug, PartialEq, Eq)]
pub enum Diagnostic {
    Warning {
        path: PathBuf,
        message: String,
    },
    Collision {
        name: String,
        winner: PathBuf,
        loser: PathBuf,
    },
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Warning { path, message } => {
                write!(formatter, "skill warning: {}: {message}", path.display())
            }
            Self::Collision {
                name,
                winner,
                loser,
            } => write!(
                formatter,
                "skill \"{name}\" collision: kept {}, skipped {}",
                winner.display(),
                loser.display()
            ),
        }
    }
}

#[derive(Debug, Default)]
pub struct Skills {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    RustClaude,
    Agents,
}

pub fn load() -> Skills {
    let config_dir = config::dir();
    let home = std::env::var_os("HOME").map(PathBuf::from);
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
    let with_scope =
        |files: Vec<PathBuf>, scope: Scope| files.into_iter().map(move |path| (path, scope));
    let project_dir = cwd.join(".rust-coding-agent");
    let mut paths = Vec::new();
    if !config_dir.is_some_and(|config_dir| same_dir(config_dir, &project_dir)) {
        paths.extend(with_scope(
            collect_skill_files(&project_dir.join("skills"), Mode::RustClaude),
            Scope::Project,
        ));
    }
    let user_agents_dir = home.map(|home| home.join(".agents").join("skills"));
    for dir in ancestor_agents_skill_dirs(cwd) {
        if !user_agents_dir
            .as_ref()
            .is_some_and(|user| same_dir(user, &dir))
        {
            paths.extend(with_scope(
                collect_skill_files(&dir, Mode::Agents),
                Scope::Project,
            ));
        }
    }
    if let Some(config_dir) = config_dir {
        paths.extend(with_scope(
            collect_skill_files(&config_dir.join("skills"), Mode::RustClaude),
            Scope::Global,
        ));
    }
    if let Some(dir) = &user_agents_dir {
        paths.extend(with_scope(
            collect_skill_files(dir, Mode::Agents),
            Scope::Global,
        ));
    }

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
            collisions.push(Diagnostic::Collision {
                name: skill.name,
                winner: existing.path.clone(),
                loser: skill.path,
            });
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
        return vec![start.join(".agents").join("skills")];
    };
    start
        .ancestors()
        .take_while(|dir| *dir != repository_root)
        .chain([repository_root])
        .map(|dir| dir.join(".agents").join("skills"))
        .collect()
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

fn load_skill_file(path: &Path, scope: Scope) -> (Option<Skill>, Vec<Diagnostic>) {
    let warning = |message: String| Diagnostic::Warning {
        path: path.to_path_buf(),
        message,
    };
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
    let mut diagnostics: Vec<Diagnostic> = validate_description(description)
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
            String::from,
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
mod tests {
    use super::*;
    use crate::tools::test_support::TemporaryDir;

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn names(loaded: &Skills) -> Vec<&str> {
        loaded
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect()
    }

    fn scopes(loaded: &Skills) -> Vec<Scope> {
        loaded.skills.iter().map(|skill| skill.scope).collect()
    }

    #[test]
    fn keeps_home_agents_skills_global_when_home_is_a_symlink() {
        let directory = TemporaryDir::new("skills-symlinked-home");
        let root = directory.path();
        let real_home = root.join("real-home");
        write(
            &real_home.join(".agents/skills/gamma/SKILL.md"),
            "---\nname: gamma\ndescription: Home gamma.\n---\n",
        );
        let home = root.join("home");
        std::os::unix::fs::symlink(&real_home, &home).unwrap();

        let loaded = load_from(
            Some(&home.join(".rust-coding-agent")),
            Some(&home),
            &real_home,
        );
        assert_eq!(scopes(&loaded), [Scope::Global]);
    }

    #[test]
    fn loads_skills_from_all_locations_and_keeps_first_on_collision() {
        let directory = TemporaryDir::new("skills-locations");
        let root = directory.path();
        let home = root.join("home");
        let project = root.join("project");
        let cwd = project.join("sub");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        write(
            &cwd.join(".rust-coding-agent/skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: Project alpha.\n---\nBody",
        );
        write(
            &project.join(".agents/skills/beta/SKILL.md"),
            "---\ndescription: Project beta.\n---\n",
        );
        write(
            &home.join(".rust-coding-agent/skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: Home alpha.\n---\n",
        );
        write(
            &home.join(".agents/skills/gamma/SKILL.md"),
            "---\nname: gamma\ndescription: >\n  Folded\n  text.\n---\n",
        );
        write(
            &root.join(".agents/skills/outside/SKILL.md"),
            "---\ndescription: Outside the repository.\n---\n",
        );

        let loaded = load_from(Some(&home.join(".rust-coding-agent")), Some(&home), &cwd);
        assert_eq!(names(&loaded), ["alpha", "beta", "gamma"]);
        assert_eq!(loaded.skills[0].description, "Project alpha.");
        assert_eq!(loaded.skills[2].description, "Folded text.");
        assert_eq!(
            scopes(&loaded),
            [Scope::Project, Scope::Project, Scope::Global]
        );
        assert_eq!(
            loaded.diagnostics,
            [Diagnostic::Collision {
                name: "alpha".into(),
                winner: cwd.join(".rust-coding-agent/skills/alpha/SKILL.md"),
                loser: home.join(".rust-coding-agent/skills/alpha/SKILL.md"),
            }]
        );
    }

    #[test]
    fn loads_agents_skills_only_from_the_current_folder_outside_a_repository() {
        let directory = TemporaryDir::new("skills-no-repository");
        let root = directory.path();
        let cwd = root.join("work");
        write(
            &cwd.join(".agents/skills/inside/SKILL.md"),
            "---\ndescription: Inside.\n---\n",
        );
        write(
            &root.join(".agents/skills/parent/SKILL.md"),
            "---\ndescription: Parent.\n---\n",
        );
        let loaded = load_from(None, None, &cwd);
        assert_eq!(names(&loaded), ["inside"]);
    }

    #[test]
    fn labels_skills_global_when_run_from_the_home_folder() {
        let directory = TemporaryDir::new("skills-home-cwd");
        let home = directory.path();
        write(
            &home.join(".rust-coding-agent/skills/alpha/SKILL.md"),
            "---\ndescription: Alpha.\n---\n",
        );
        let loaded = load_from(Some(&home.join(".rust-coding-agent")), Some(home), home);
        assert_eq!(scopes(&loaded), [Scope::Global]);
    }

    #[test]
    fn names_loose_markdown_skills_after_their_file() {
        let directory = TemporaryDir::new("skills-loose-names");
        let root = directory.path();
        let home = root.join("home");
        let cwd = root.join("project");
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        let skills = home.join(".rust-coding-agent/skills");
        write(
            &skills.join("commit.md"),
            "---\ndescription: Commit.\n---\n",
        );
        write(
            &skills.join("review.md"),
            "---\ndescription: Review.\n---\n",
        );
        write(
            &skills.join("My Notes.md"),
            "---\ndescription: Notes.\n---\n",
        );

        let loaded = load_from(Some(&home.join(".rust-coding-agent")), Some(&home), &cwd);
        assert_eq!(names(&loaded), ["My Notes", "commit", "review"]);
        assert!(loaded.diagnostics.iter().all(|diagnostic| matches!(
            diagnostic,
            Diagnostic::Warning { path, .. } if *path == skills.join("My Notes.md")
        )));
        assert!(!loaded.diagnostics.is_empty());
    }

    #[test]
    fn follows_discovery_rules() {
        let directory = TemporaryDir::new("skills-discovery");
        let root = directory.path();
        let dir = root.join("skills");
        write(&dir.join("loose.md"), "---\ndescription: Loose.\n---\n");
        write(&dir.join("notes.md"), "No frontmatter");
        write(
            &dir.join("nested/inner.md"),
            "---\ndescription: Inner.\n---\n",
        );
        write(
            &dir.join("outer/SKILL.md"),
            "---\ndescription: Outer.\n---\n",
        );
        write(
            &dir.join("outer/deep/SKILL.md"),
            "---\ndescription: Deep.\n---\n",
        );
        write(
            &dir.join(".hidden/SKILL.md"),
            "---\ndescription: Hidden.\n---\n",
        );
        write(
            &dir.join("node_modules/x/SKILL.md"),
            "---\ndescription: X.\n---\n",
        );
        write(
            &dir.join("ignored/SKILL.md"),
            "---\ndescription: Ignored.\n---\n",
        );
        write(&dir.join(".gitignore"), "ignored/\n");

        let files = collect_skill_files(&dir, Mode::RustClaude);
        assert_eq!(
            files,
            [
                dir.join("loose.md"),
                dir.join("notes.md"),
                dir.join("outer/SKILL.md")
            ]
        );
        let files = collect_skill_files(&dir, Mode::Agents);
        assert_eq!(
            files,
            [dir.join("nested/inner.md"), dir.join("outer/SKILL.md")]
        );
    }

    #[test]
    fn applies_ignore_files_relative_to_their_own_directory() {
        let directory = TemporaryDir::new("skills-nested-ignore");
        let root = directory.path();
        let dir = root.join("skills");
        for skill in [
            "old",
            "scratch",
            "team/drafts",
            "team/old",
            "team/scratch",
            "team/x/drafts",
            "team/x/live",
        ] {
            write(
                &dir.join(skill).join("SKILL.md"),
                "---\ndescription: Skill.\n---\n",
            );
        }
        write(&dir.join(".gitignore"), "/old\nscratch/\n");
        write(&dir.join("team/.gitignore"), "drafts/\n!scratch/\n");

        let files = collect_skill_files(&dir, Mode::Agents);
        assert_eq!(
            files,
            [
                dir.join("team/old/SKILL.md"),
                dir.join("team/scratch/SKILL.md"),
                dir.join("team/x/live/SKILL.md"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn visits_each_folder_once_through_symlink_loops() {
        let directory = TemporaryDir::new("skills-symlink-loop");
        let root = directory.path();
        let dir = root.join("skills");
        write(&dir.join("a/SKILL.md"), "---\ndescription: A.\n---\n");
        write(&dir.join("team/b/SKILL.md"), "---\ndescription: B.\n---\n");
        std::os::unix::fs::symlink("..", dir.join("team/loop")).unwrap();

        let files = collect_skill_files(&dir, Mode::Agents);
        assert_eq!(files, [dir.join("a/SKILL.md"), dir.join("team/b/SKILL.md")]);
    }

    #[test]
    fn validates_skill_files() {
        let directory = TemporaryDir::new("skills-validation");
        let root = directory.path();
        let missing = root.join("missing/SKILL.md");
        write(&missing, "---\nname: missing\n---\n");
        let (skill, diagnostics) = load_skill_file(&missing, Scope::Project);
        assert!(skill.is_none());
        assert_eq!(
            diagnostics,
            [Diagnostic::Warning {
                path: missing,
                message: "description is required".into(),
            }]
        );

        let invalid = root.join("Bad--Name/SKILL.md");
        write(
            &invalid,
            "---\ndescription: Still loads.\ndisable-model-invocation: true\n---\n",
        );
        let (skill, diagnostics) = load_skill_file(&invalid, Scope::Project);
        let skill = skill.unwrap();
        assert_eq!(skill.name, "Bad--Name");
        assert!(skill.disable_model_invocation);
        assert_eq!(diagnostics.len(), 2);

        let broken = root.join("broken/SKILL.md");
        write(&broken, "---\ndescription: [unclosed\n---\n");
        let (skill, diagnostics) = load_skill_file(&broken, Scope::Project);
        assert!(skill.is_none());
        assert_eq!(diagnostics.len(), 1);
    }
}
