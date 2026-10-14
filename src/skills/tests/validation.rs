use super::*;

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
fn skips_loose_markdown_with_broken_frontmatter_without_a_warning() {
    let directory = TemporaryDir::new("skills-loose-broken");
    let root = directory.path();
    let home = root.join("home");
    let cwd = root.join("project");
    std::fs::create_dir_all(cwd.join(".git")).unwrap();
    let skills = home.join(".rust-coding-agent/skills");
    write(
        &skills.join("notes.md"),
        "---\ndescription: [unclosed\n---\n",
    );
    write(
        &skills.join("commit.md"),
        "---\ndescription: Commit.\n---\n",
    );

    let loaded = load_from(Some(&home.join(".rust-coding-agent")), Some(&home), &cwd);
    assert_eq!(names(&loaded), ["commit"]);
    assert_eq!(loaded.diagnostics, Vec::<String>::new());
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
