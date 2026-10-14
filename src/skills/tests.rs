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
        [format!(
            "skill \"alpha\" collision: kept {}, skipped {}",
            cwd.join(".rust-coding-agent/skills/alpha/SKILL.md")
                .display(),
            home.join(".rust-coding-agent/skills/alpha/SKILL.md")
                .display()
        )]
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
    let prefix = format!("skill warning: {}: ", skills.join("My Notes.md").display());
    assert!(
        loaded
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.starts_with(&prefix))
    );
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
        [format!(
            "skill warning: {}: description is required",
            missing.display()
        )]
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
    let prefix = format!("skill warning: {}: ", invalid.display());
    assert_eq!(
        diagnostics,
        [
            format!(
                "{prefix}name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
            ),
            format!("{prefix}name must not contain consecutive hyphens"),
        ]
    );

    let broken = root.join("broken/SKILL.md");
    write(&broken, "---\ndescription: [unclosed\n---\n");
    let (skill, diagnostics) = load_skill_file(&broken, Scope::Project);
    assert!(skill.is_none());
    assert_eq!(diagnostics.len(), 1);
}

#[test]
fn reads_frontmatter_written_with_windows_line_endings() {
    let directory = TemporaryDir::new("skills-crlf");
    let path = directory.path().join("deploy/SKILL.md");
    write(
        &path,
        "\u{feff}---\r\nname: deploy\r\ndescription: Ships it.\r\n---\r\nBuild.\r\nPush.\rTag.\r\n",
    );
    let (skill, diagnostics) = load_skill_file(&path, Scope::Project);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let skill = skill.unwrap();
    assert_eq!(
        (skill.name.as_str(), skill.description.as_str()),
        ("deploy", "Ships it.")
    );
    let expanded = expand_command("/skill:deploy", &[skill]).unwrap();
    assert!(
        expanded.ends_with("\n\nBuild.\nPush.\nTag.\n</skill>"),
        "{expanded:?}"
    );
}

#[test]
fn warns_about_each_rule_a_skill_breaks() {
    let directory = TemporaryDir::new("skills-rules");
    let path = directory.path().join("x/SKILL.md");
    let long_name = format!("-{}", "a".repeat(64));
    write(
        &path,
        &format!(
            "---\nname: {long_name}\ndescription: {}\n---\n",
            "d".repeat(1025)
        ),
    );
    let (skill, diagnostics) = load_skill_file(&path, Scope::Project);
    assert_eq!(skill.unwrap().name, long_name);
    let prefix = format!("skill warning: {}: ", path.display());
    assert_eq!(
        diagnostics,
        [
            format!("{prefix}description exceeds 1024 characters (1025)"),
            format!("{prefix}name exceeds 64 characters (65)"),
            format!("{prefix}name must not start or end with a hyphen"),
        ]
    );
}
