use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde_json::Value;

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;
const IGNORE_FILE_NAMES: [&str; 3] = [".gitignore", ".ignore", ".fdignore"];
const SKILL_FILE: &str = "SKILL.md";
const COMMAND_PREFIX: &str = "/skill:";

#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub base_dir: PathBuf,
    pub disable_model_invocation: bool,
    pub scope: Scope,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scope {
    Global,
    Project,
}

impl fmt::Display for Scope {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(match self {
            Scope::Global => "global",
            Scope::Project => "project",
        })
    }
}

#[derive(Debug, PartialEq)]
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
            Diagnostic::Warning { path, message } => {
                write!(formatter, "skill warning: {}: {message}", path.display())
            }
            Diagnostic::Collision {
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
    let config_dir = crate::config::dir();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().unwrap_or_default();
    load_from(config_dir.as_deref(), home.as_deref(), &cwd)
}

/// Whether two paths name the same folder, so a project `.rust-claude` run
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
    let project_dir = cwd.join(".rust-claude");
    let mut paths: Vec<(PathBuf, Scope)> = Vec::new();
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
        match skills.iter().find(|existing| existing.name == skill.name) {
            Some(existing) => collisions.push(Diagnostic::Collision {
                name: skill.name,
                winner: existing.path.clone(),
                loser: skill.path,
            }),
            None => {
                real_paths.insert(real_path);
                skills.push(skill);
            }
        }
    }
    diagnostics.extend(collisions);
    Skills {
        skills,
        diagnostics,
    }
}

fn ancestor_agents_skill_dirs(start: &Path) -> Vec<PathBuf> {
    let repository_root = start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf);
    let mut dirs = Vec::new();
    for dir in start.ancestors() {
        dirs.push(dir.join(".agents").join("skills"));
        if repository_root.as_deref() == Some(dir) {
            break;
        }
    }
    dirs
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
    entries.sort_by_key(|entry| entry.file_name());

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
    let normalised = content.replace("\r\n", "\n").replace('\r', "\n");
    if !normalised.starts_with("---") {
        return (None, normalised);
    }
    let Some(end) = normalised[3..].find("\n---").map(|index| index + 3) else {
        return (None, normalised);
    };
    let yaml = normalised.get(4..end).unwrap_or_default().to_string();
    let body = normalised[end + 4..].trim().to_string();
    (Some(yaml).filter(|yaml| !yaml.is_empty()), body)
}

fn parse_frontmatter(content: &str) -> Result<Value> {
    match split_frontmatter(content).0 {
        Some(yaml) => {
            let value: Value = yaml_serde::from_str(&yaml)?;
            Ok(if value.is_null() {
                Value::Object(Default::default())
            } else {
                value
            })
        }
        None => Ok(Value::Object(Default::default())),
    }
}

fn strip_frontmatter(content: &str) -> String {
    split_frontmatter(content).1
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
    match description {
        Some(description) if !description.trim().is_empty() => {
            let length = description.chars().count();
            if length > MAX_DESCRIPTION_LENGTH {
                vec![format!(
                    "description exceeds {MAX_DESCRIPTION_LENGTH} characters ({length})"
                )]
            } else {
                Vec::new()
            }
        }
        _ => vec!["description is required".into()],
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
    let description = frontmatter["description"].as_str();
    let has_description = description.is_some_and(|description| !description.trim().is_empty());
    if !is_declared_skill && !has_description {
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
        .map(String::from)
        .unwrap_or_else(|| {
            let fallback = if is_declared_skill {
                base_dir.file_name()
            } else {
                path.file_stem()
            };
            fallback
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    diagnostics.extend(validate_name(&name).into_iter().map(&warning));
    let Some(description) = description.filter(|_| has_description) else {
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

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
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
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|skill| !skill.disable_model_invocation)
        .collect();
    if visible.is_empty() {
        return None;
    }
    let mut lines = vec![
        "The following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".into(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".into(),
        String::new(),
        "<available_skills>".into(),
    ];
    for skill in visible {
        lines.push("  <skill>".into());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.path.to_string_lossy())
        ));
        lines.push("  </skill>".into());
    }
    lines.push("</available_skills>".into());
    Some(lines.join("\n"))
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
    let body = strip_frontmatter(&content);
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

#[derive(Debug, PartialEq)]
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
    let user_message = match rest.find("\n</skill>\n\n") {
        Some(index) => Some(rest[index + "\n</skill>\n\n".len()..].trim()),
        None if rest.ends_with("\n</skill>") => None,
        None => return None,
    };
    Some(SkillBlock {
        name: unescape_xml(name),
        user_message: user_message.filter(|message| !message.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rust-claude-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn keeps_home_agents_skills_global_when_home_is_a_symlink() {
        let root = temp_dir("symlinked-home");
        let real_home = root.join("real-home");
        write(
            &real_home.join(".agents/skills/gamma/SKILL.md"),
            "---\nname: gamma\ndescription: Home gamma.\n---\n",
        );
        let home = root.join("home");
        std::os::unix::fs::symlink(&real_home, &home).unwrap();

        let loaded = load_from(Some(&home.join(".rust-claude")), Some(&home), &real_home);
        let scopes: Vec<Scope> = loaded.skills.iter().map(|skill| skill.scope).collect();
        assert_eq!(scopes, [Scope::Global]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn loads_skills_from_all_locations_and_keeps_first_on_collision() {
        let root = temp_dir("locations");
        let home = root.join("home");
        let project = root.join("project");
        let cwd = project.join("sub");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        write(
            &cwd.join(".rust-claude/skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: Project alpha.\n---\nBody",
        );
        write(
            &project.join(".agents/skills/beta/SKILL.md"),
            "---\ndescription: Project beta.\n---\n",
        );
        write(
            &home.join(".rust-claude/skills/alpha/SKILL.md"),
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

        let loaded = load_from(Some(&home.join(".rust-claude")), Some(&home), &cwd);
        let names: Vec<&str> = loaded
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
        assert_eq!(loaded.skills[0].description, "Project alpha.");
        assert_eq!(loaded.skills[2].description, "Folded text.");
        let scopes: Vec<Scope> = loaded.skills.iter().map(|skill| skill.scope).collect();
        assert_eq!(scopes, [Scope::Project, Scope::Project, Scope::Global]);
        assert_eq!(
            loaded.diagnostics,
            [Diagnostic::Collision {
                name: "alpha".into(),
                winner: cwd.join(".rust-claude/skills/alpha/SKILL.md"),
                loser: home.join(".rust-claude/skills/alpha/SKILL.md"),
            }]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn labels_skills_global_when_run_from_the_home_folder() {
        let home = temp_dir("home-cwd");
        write(
            &home.join(".rust-claude/skills/alpha/SKILL.md"),
            "---\ndescription: Alpha.\n---\n",
        );
        let loaded = load_from(Some(&home.join(".rust-claude")), Some(&home), &home);
        let _ = std::fs::remove_dir_all(&home);
        let scopes: Vec<Scope> = loaded.skills.iter().map(|skill| skill.scope).collect();
        assert_eq!(scopes, [Scope::Global]);
    }

    #[test]
    fn names_loose_markdown_skills_after_their_file() {
        let root = temp_dir("loose-names");
        let home = root.join("home");
        let cwd = root.join("project");
        std::fs::create_dir_all(cwd.join(".git")).unwrap();
        let skills = home.join(".rust-claude/skills");
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

        let loaded = load_from(Some(&home.join(".rust-claude")), Some(&home), &cwd);
        let names: Vec<&str> = loaded
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(names, ["My Notes", "commit", "review"]);
        assert!(loaded.diagnostics.iter().all(|diagnostic| matches!(
            diagnostic,
            Diagnostic::Warning { path, .. } if *path == skills.join("My Notes.md")
        )));
        assert!(!loaded.diagnostics.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn follows_discovery_rules() {
        let root = temp_dir("discovery");
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn applies_ignore_files_relative_to_their_own_directory() {
        let root = temp_dir("nested-ignore");
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn visits_each_folder_once_through_symlink_loops() {
        let root = temp_dir("symlink-loop");
        let dir = root.join("skills");
        write(&dir.join("a/SKILL.md"), "---\ndescription: A.\n---\n");
        write(&dir.join("team/b/SKILL.md"), "---\ndescription: B.\n---\n");
        std::os::unix::fs::symlink("..", dir.join("team/loop")).unwrap();

        let files = collect_skill_files(&dir, Mode::Agents);
        assert_eq!(files, [dir.join("a/SKILL.md"), dir.join("team/b/SKILL.md")]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn validates_skill_files() {
        let root = temp_dir("validation");
        let missing = root.join("missing/SKILL.md");
        write(&missing, "---\nname: missing\n---\n");
        let (skill, diagnostics) = load_skill_file(&missing, Scope::Project);
        assert!(skill.is_none());
        assert_eq!(
            diagnostics,
            [Diagnostic::Warning {
                path: missing.clone(),
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
        let _ = std::fs::remove_dir_all(root);
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
    fn expands_and_parses_skill_commands() {
        let root = temp_dir("expand");
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
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn parses_skill_blocks_with_quotes_in_name_and_path() {
        let root = temp_dir("expand-quotes");
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
        let _ = std::fs::remove_dir_all(root);
    }
}
