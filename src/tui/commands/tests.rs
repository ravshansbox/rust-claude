use crate::auth::now_millis;
use crate::mcp::{
    self,
    tests::sign_in::{AUTH_FILE, save_tokens, save_tokens_as},
};
use crate::skills::{Scope, Skill};
use crate::tui::{
    App,
    keys::{Action, handle_input},
    test_support::{new_app, press, press_with, screen},
};
use crossterm::event::{Event, KeyCode, KeyModifiers};

#[test]
fn lists_commands_and_skill_commands_in_alphabetical_order() {
    let mut app = new_app();
    app.skills = vec![Skill {
        name: "lint".into(),
        description: "lint skill".into(),
        path: "/skills/lint/SKILL.md".into(),
        base_dir: "/skills/lint".into(),
        disable_model_invocation: false,
        scope: Scope::Global,
    }];
    handle_input(Event::Paste("/".into()), &mut app, |_| {});
    let shown = screen(&mut app);
    let names = [
        "/compact",
        "/effort",
        "/mcp",
        "/model",
        "/new",
        "/quit",
        "/resume",
        "/skill:lint",
    ];
    let positions: Vec<usize> = names
        .iter()
        .map(|name| shown.find(&format!("{name} ")).expect(name))
        .collect();
    assert!(positions.is_sorted(), "{shown}");
}

fn suggested(app: &App) -> Vec<String> {
    app.visible_suggestions()
        .map(|suggestions| {
            suggestions
                .items
                .into_iter()
                .map(|(name, _)| name)
                .collect()
        })
        .unwrap_or_default()
}

fn sign_in_servers(names: &[&str]) -> Vec<mcp::SignInServer> {
    names
        .iter()
        .map(|name| mcp::SignInServer {
            name: name.to_string(),
            url: format!("https://{name}.example/mcp"),
        })
        .collect()
}

#[test]
fn completes_mcp_subcommands_and_server_names() {
    let _file = AUTH_FILE.blocking_read();
    save_tokens(
        "https://linear.example/mcp",
        "token",
        now_millis() + 3_600_000,
    );
    let mut app = new_app();
    app.mcp_sign_in_servers = sign_in_servers(&["figma", "files", "linear"]);
    handle_input(Event::Paste("/mc".into()), &mut app, |_| {});
    assert_eq!(suggested(&app), ["/mcp"]);
    handle_input(Event::Paste("p ".into()), &mut app, |_| {});
    assert_eq!(suggested(&app), ["/mcp login", "/mcp logout"]);
    handle_input(Event::Paste("login f".into()), &mut app, |_| {});
    assert_eq!(suggested(&app), ["/mcp login figma", "/mcp login files"]);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.input, "/mcp login figma");
    let mut signing_in = Vec::new();
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::McpSignIn(name) = action {
            signing_in.push(name);
        }
    });
    assert_eq!(signing_in, ["figma"]);
    assert_eq!(app.input, "");
    handle_input(Event::Paste("/mcp logout l".into()), &mut app, |_| {});
    assert_eq!(suggested(&app), ["/mcp logout linear"]);
    let mut signing_out = Vec::new();
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::McpSignOut(name) = action {
            signing_out.push(name);
        }
    });
    assert_eq!(signing_out, ["linear"]);
    handle_input(Event::Paste("/mcp logout ".into()), &mut app, |_| {});
    assert_eq!(suggested(&app), ["/mcp logout linear"]);
}

#[test]
fn marks_signed_in_servers_after_mcp_login() {
    let _file = AUTH_FILE.blocking_read();
    let expires_at = now_millis() + 3_600_000;
    save_tokens("https://notion.example/mcp", "token", expires_at);
    save_tokens_as(
        "https://slack.example/mcp",
        "token",
        expires_at,
        Some("alice"),
    );
    let mut app = new_app();
    app.mcp_sign_in_servers = sign_in_servers(&["github", "notion", "slack"]);
    handle_input(Event::Paste("/mcp login ".into()), &mut app, |_| {});
    let shown = screen(&mut app);
    let line = |name: &str| {
        shown
            .lines()
            .find(|line| line.contains(&format!("/mcp login {name}")))
            .expect(name)
            .trim_end()
            .to_string()
    };
    assert!(line("github").ends_with("/mcp login github"), "{shown}");
    assert!(line("notion").ends_with("signed in"), "{shown}");
    assert!(line("slack").ends_with("signed in"), "{shown}");
    assert!(!shown.contains("alice"), "{shown}");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.input, "/mcp login notion");
}

#[test]
fn explains_how_to_use_the_mcp_command() {
    let mut app = new_app();
    for input in ["/mcp", "/mcp login", "/mcp logout", "/mcp dance figma"] {
        handle_input(Event::Paste(input.into()), &mut app, |_| {});
        app.commands_dismissed = true;
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.messages.last().unwrap().text,
            "usage: /mcp login <server> or /mcp logout <server>",
            "{input}"
        );
        assert_eq!(app.input, input);
        app.input.clear();
        app.cursor = 0;
    }
}

#[test]
fn wraps_the_command_selection_at_either_end() {
    let mut app = new_app();
    handle_input(Event::Paste("/".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.input, "/resume");
    app.input.clear();
    app.cursor = 0;
    handle_input(Event::Paste("/".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Tab);
    assert_eq!(app.input, "/compact");
}

#[test]
fn runs_the_skill_named_exactly_when_a_longer_name_comes_first() {
    let skill = |name: &str, scope| Skill {
        name: name.into(),
        description: format!("{name} skill"),
        path: format!("/skills/{name}/SKILL.md").into(),
        base_dir: format!("/skills/{name}").into(),
        disable_model_invocation: false,
        scope,
    };
    let mut app = new_app();
    app.skills = vec![
        skill("pdf-tools", Scope::Project),
        skill("pdf", Scope::Global),
    ];
    handle_input(Event::Paste("/skill:pdf".into()), &mut app, |_| {});
    let mut submitted = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Submit(prompt, _, _) = action {
            submitted = Some(prompt);
        }
    });
    assert_eq!(submitted.as_deref(), Some("/skill:pdf"));
    assert_eq!(app.messages.last().unwrap().text, "[skill] pdf");
}
