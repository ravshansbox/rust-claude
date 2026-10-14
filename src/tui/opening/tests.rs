use super::{opening_app, show_skills_and_instructions};
use crate::{
    agent::test_support::{self, MockApi, text_reply},
    mcp,
    skills::{Scope, Skill, Skills},
    tui::test_support::new_app,
};
use std::path::PathBuf;

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
        diagnostics: vec!["skill warning: broken/SKILL.md: missing description".into()],
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

#[tokio::test]
async fn opens_a_continued_session_with_its_conversation_then_what_loaded() {
    let api = MockApi::start(vec![text_reply("Hi there")]).await;
    let mut agent = test_support::agent(&api);
    agent.prompt("hello", &[], |_| {}).await.unwrap();
    let id = agent.session.id.clone();
    drop(agent);
    let mut continued = test_support::agent(&api);
    let resumed = continued.resume(&id).await;
    test_support::remove_session(&continued);
    resumed.unwrap();
    let startup = mcp::Startup {
        diagnostics: vec!["mcp warning: broken.json".into()],
        servers: Vec::new(),
        overridden: Vec::new(),
        sign_in_servers: Vec::new(),
    };
    let app = opening_app(&continued, &startup, None);
    let shown: Vec<&str> = app.messages[1..6]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        shown,
        [
            "mcp warning: broken.json",
            "hello",
            "Hi there",
            "continued session",
            "loaded tools: agent, bash, bash_stop, edit, read, web_fetch, web_search, write",
        ]
    );
}
