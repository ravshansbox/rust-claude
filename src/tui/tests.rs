use super::{
    PathBuf, Request, UiEvent, git_branch, history_writer, save_changed_settings,
    show_skills_and_instructions, sign_out_of_mcp_server, test_support::new_app, tools_line,
};
use crate::{
    agent::AgentEvent,
    config, history,
    settings::Settings,
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

#[test]
fn saves_only_the_model_and_effort_the_user_changed() {
    let path = config::dir().unwrap().join("settings.json");
    std::fs::write(&path, r#"{ "effort": "low", "notifications": true }"#).unwrap();
    let mut app = new_app();
    let (mut saved_model, mut saved_effort) = (app.model.clone(), app.effort);
    app.set_model("claude-haiku-5-5");
    save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
    let after_model = Settings::load();
    app.set_effort("high");
    save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
    let after_effort = Settings::load();
    std::fs::write(&path, "{ broken").unwrap();
    app.set_effort("max");
    save_changed_settings(&mut app, &mut saved_model, &mut saved_effort);
    let broken = std::fs::read_to_string(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(after_model.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(after_model.effort.as_deref(), Some("low"));
    assert_eq!(after_model.notifications, Some(true));
    assert_eq!(after_effort.model.as_deref(), Some("claude-haiku-5-5"));
    assert_eq!(after_effort.effort.as_deref(), Some("high"));
    assert_eq!(broken, "{ broken");
    let last = &app.messages.last().unwrap().text;
    assert!(last.starts_with("failed to save settings: "), "{last}");
}

#[tokio::test]
async fn restarts_an_mcp_server_after_signing_out() {
    let path = config::dir().unwrap().join("mcp.json");
    std::fs::write(
        &path,
        r#"{ "mcpServers": { "web": { "url": "http://127.0.0.1:9/mcp", "timeout": 5 } } }"#,
    )
    .unwrap();
    let (events, mut shown) = tokio::sync::mpsc::unbounded_channel();
    let (requests, mut sent) = tokio::sync::mpsc::unbounded_channel();
    sign_out_of_mcp_server("web".into(), events.clone(), requests.clone()).await;
    sign_out_of_mcp_server("missing".into(), events, requests).await;
    std::fs::remove_file(&path).unwrap();
    match shown.try_recv() {
        Ok(UiEvent::McpRestarting { name, notice, .. }) => {
            assert_eq!(name, "web");
            assert_eq!(notice, "MCP server web was not signed in");
        }
        _ => panic!("the server did not restart"),
    }
    assert!(matches!(sent.try_recv(), Ok(Request::ReplaceMcpServer(_))));
    match shown.try_recv() {
        Ok(UiEvent::Agent(AgentEvent::Notice(notice))) => assert_eq!(
            notice,
            "MCP sign-out of missing failed: no MCP server named missing"
        ),
        _ => panic!("no notice for the missing server"),
    }
    assert!(sent.try_recv().is_err());
}
