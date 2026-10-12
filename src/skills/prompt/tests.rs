use super::*;
use crate::tools::test_support::TemporaryDir;
use std::path::Path;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
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
fn lists_global_skills_before_project_ones_in_prompt() {
    let mut project = skill("project", Path::new("/skills/project"), false);
    project.scope = Scope::Project;
    let global = skill("global", Path::new("/skills/global"), false);
    let prompt = format_for_prompt(&[project, global]).unwrap();
    assert!(prompt.find("<name>global").unwrap() < prompt.find("<name>project").unwrap());
}

#[test]
fn expands_and_parses_skill_commands() {
    let directory = TemporaryDir::new("skills-expand");
    let root = directory.path();
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
}

#[test]
fn parses_skill_blocks_whose_body_shows_a_skill_block() {
    let directory = TemporaryDir::new("skills-expand-nested");
    let root = directory.path();
    let dir = root.join("demo");
    write(
        &dir.join("SKILL.md"),
        "---\ndescription: Demo.\n---\nA call looks like:\n\n<skill name=\"x\" location=\"y\">\nBody\n</skill>\n\nrequest\n\nDo it.\n",
    );
    let skills = [skill("demo", &dir, true)];
    let expanded = expand_command("/skill:demo fix the bug", &skills).unwrap();
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
}

#[test]
fn parses_skill_blocks_with_quotes_in_name_and_path() {
    let directory = TemporaryDir::new("skills-expand-quotes");
    let root = directory.path();
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
}
