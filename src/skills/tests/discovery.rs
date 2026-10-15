use super::*;

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
