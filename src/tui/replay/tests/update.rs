use crate::tui::{
    Role, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, screen, screen_of_width},
};
use crate::update::Progress;
use std::time::Duration;

#[test]
fn adds_each_update_step_to_the_version_line() {
    let mut app = new_app();
    app.push(Role::Event, "loaded AGENTS.md");
    let seconds = Duration::from_secs;
    for progress in [
        Progress::Available("v0.2.0".into()),
        Progress::Downloading,
        Progress::Downloaded(seconds(3)),
        Progress::Building,
        Progress::Built(seconds(125)),
        Progress::Ready,
    ] {
        handle_agent_event(UiEvent::Update(progress), &mut app);
    }
    let shown = screen_of_width(&mut app, 160);
    let version = shown
        .find(concat!(
            "Rust Coding Agent v",
            env!("CARGO_PKG_VERSION"),
            " · update v0.2.0 available · downloaded in 3s · built in 2m 5s",
            " · installed, restart rust-coding-agent to use it"
        ))
        .expect(&shown);
    assert!(
        version < shown.find("loaded AGENTS.md").expect(&shown),
        "{shown}"
    );
}

#[test]
fn replaces_the_failed_step_with_why_it_failed() {
    let mut app = new_app();
    let seconds = Duration::from_secs;
    for progress in [
        Progress::Available("v0.2.0".into()),
        Progress::Downloading,
        Progress::Downloaded(seconds(3)),
        Progress::Building,
        Progress::BuildFailed("error: x".into(), seconds(70)),
    ] {
        handle_agent_event(UiEvent::Update(progress), &mut app);
    }
    let shown = screen_of_width(&mut app, 160);
    assert!(
        shown.contains(concat!(
            "Rust Coding Agent v",
            env!("CARGO_PKG_VERSION"),
            " · update v0.2.0 available · downloaded in 3s",
            " · build failed after 1m 10s: error: x"
        )),
        "{shown}"
    );
}

#[test]
fn shows_update_progress_with_the_version_after_clearing() {
    let mut app = new_app();
    handle_agent_event(
        UiEvent::Update(Progress::Available("v0.2.0".into())),
        &mut app,
    );
    app.clear_session();
    handle_agent_event(UiEvent::Update(Progress::Downloading), &mut app);
    let shown = screen(&mut app);
    assert!(
        shown.contains(concat!(
            "Rust Coding Agent v",
            env!("CARGO_PKG_VERSION"),
            " · update v0.2.0 available · downloading"
        )),
        "{shown}"
    );
}
