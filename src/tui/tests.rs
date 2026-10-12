use super::{
    PathBuf, UiEvent, git_branch, history_writer, show_skills_and_instructions,
    test_support::new_app, tools_line,
};
use crate::{
    agent::AgentEvent,
    history,
    skills::{Diagnostic, Scope, Skill, Skills},
    tools::test_support::TemporaryDir,
};

#[test]
fn lists_loaded_skills_by_scope_then_their_problems_then_instructions() {
    let skill = |name: &str, scope| Skill {
        name: name.into(),
        description: String::new(),
        path: PathBuf::new(),
        base_dir: PathBuf::new(),
        disable_model_invocation: false,
        scope,
    };
    let skills = Skills {
        skills: vec![
            skill("review", Scope::Project),
            skill("commit", Scope::Global),
            skill("deploy", Scope::Project),
        ],
        diagnostics: vec![Diagnostic::Warning {
            path: "broken/SKILL.md".into(),
            message: "missing description".into(),
        }],
    };
    let mut app = new_app();
    show_skills_and_instructions(&mut app, &skills, ["~/AGENTS.md", "./AGENTS.md"]);
    let shown: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        shown,
        [
            "loaded global skills: commit",
            "loaded project skills: review, deploy",
            "skill warning: broken/SKILL.md: missing description",
            "loaded ~/AGENTS.md, ./AGENTS.md",
        ]
    );
    let mut app = new_app();
    show_skills_and_instructions(&mut app, &Skills::default(), []);
    assert_eq!(app.messages.len(), 1);
}

#[test]
fn shows_the_branch_of_a_repository_with_no_commits() {
    let directory = TemporaryDir::new("unborn-branch");
    let status = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(directory.path())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(git_branch(directory.path()).as_deref(), Some("main"));
}

#[test]
fn lists_built_in_tools_on_one_line() {
    assert_eq!(
        tools_line(),
        "loaded tools: agent, bash, bash_stop, edit, read, web_search, write"
    );
}

#[test]
fn saves_prompt_history_in_order_and_reports_failures() {
    let directory = TemporaryDir::new("history-writer");
    let folder = directory.path();
    let path = folder.join("history.jsonl");
    std::fs::write(folder.join("missing"), "a file, not a folder").unwrap();
    let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
    let (sender, writer) = history_writer(events);
    for prompt in ["one", "two"] {
        sender
            .send((path.clone(), history::Entry::here(prompt)))
            .unwrap();
    }
    sender
        .send((
            folder.join("missing").join("history.jsonl"),
            history::Entry::here("three"),
        ))
        .unwrap();
    drop(sender);
    writer.join().unwrap();
    let saved: Vec<String> = history::load(&path)
        .into_iter()
        .map(|entry| entry.prompt)
        .collect();
    let notice = match received.try_recv() {
        Ok(UiEvent::Agent(AgentEvent::Notice(notice))) => notice,
        _ => String::new(),
    };
    assert_eq!(saved, ["one", "two"]);
    assert!(
        notice.starts_with("failed to save prompt history: "),
        "{notice}"
    );
}
