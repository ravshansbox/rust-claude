use super::*;
use crate::tools::test_support::TemporaryDir;

mod discovery;
mod validation;

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
        [format!(
            "skill \"alpha\" collision: kept {}, skipped {}",
            cwd.join(".rust-coding-agent/skills/alpha/SKILL.md")
                .display(),
            home.join(".rust-coding-agent/skills/alpha/SKILL.md")
                .display()
        )]
    );
}

#[cfg(unix)]
#[test]
fn loads_a_skill_linked_into_two_locations_once_without_a_collision() {
    let directory = TemporaryDir::new("skills-linked-twice");
    let root = directory.path();
    let home = root.join("home");
    let cwd = root.join("project");
    let shared = home.join(".agents/skills/shared");
    write(&shared.join("SKILL.md"), "---\ndescription: Shared.\n---\n");
    std::fs::create_dir_all(cwd.join(".rust-coding-agent/skills")).unwrap();
    std::os::unix::fs::symlink(&shared, cwd.join(".rust-coding-agent/skills/shared")).unwrap();

    let loaded = load_from(None, Some(&home), &cwd);
    assert_eq!(names(&loaded), ["shared"]);
    assert_eq!(scopes(&loaded), [Scope::Project]);
    assert!(loaded.diagnostics.is_empty(), "{:?}", loaded.diagnostics);
}

#[cfg(unix)]
#[test]
fn warns_once_about_a_skill_linked_into_two_locations() {
    let directory = TemporaryDir::new("skills-linked-twice-warning");
    let root = directory.path();
    let home = root.join("home");
    let cwd = root.join("project");
    let shared = home.join(".agents/skills/shared");
    write(
        &shared.join("SKILL.md"),
        "---\nname: Shared\ndescription: Shared.\n---\n",
    );
    std::fs::create_dir_all(cwd.join(".rust-coding-agent/skills")).unwrap();
    std::os::unix::fs::symlink(&shared, cwd.join(".rust-coding-agent/skills/shared")).unwrap();

    let loaded = load_from(None, Some(&home), &cwd);
    assert_eq!(names(&loaded), ["Shared"]);
    assert_eq!(loaded.diagnostics.len(), 1, "{:?}", loaded.diagnostics);
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
    let prefix = format!("skill warning: {}: ", skills.join("My Notes.md").display());
    assert!(
        loaded
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.starts_with(&prefix))
    );
    assert_ne!(loaded.diagnostics, Vec::<String>::new());
}
