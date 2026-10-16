use crate::tui::{
    Activity, Role, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, screen, screen_of_width},
};
use crate::update::Progress;
use std::{path::PathBuf, time::Duration};

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
        Progress::Ready(None),
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

fn installed(app: &mut crate::tui::App) {
    handle_agent_event(
        UiEvent::Update(Progress::Ready(Some(PathBuf::from("/bin/rca-new")))),
        app,
    );
}

#[test]
fn restarts_once_the_update_is_installed_and_the_app_is_idle() {
    let mut app = new_app();
    assert_eq!(app.restart_due(), None);
    installed(&mut app);
    assert!(
        screen_of_width(&mut app, 160).contains("installed, restarting when idle"),
        "{}",
        screen_of_width(&mut app, 160)
    );
    assert_eq!(app.restart_due(), Some(PathBuf::from("/bin/rca-new")));
}

#[test]
fn waits_for_the_reply_before_restarting() {
    let mut app = new_app();
    app.start(Activity::Working);
    installed(&mut app);
    assert_eq!(app.restart_due(), None);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert_eq!(app.restart_due(), Some(PathBuf::from("/bin/rca-new")));
}

#[test]
fn waits_for_queued_prompts_before_restarting() {
    let mut app = new_app();
    installed(&mut app);
    app.queue.lock().unwrap().push(crate::agent::Queued {
        prompt: "next".into(),
        images: Vec::new(),
    });
    assert_eq!(app.restart_due(), None);
    app.queue.lock().unwrap().clear();
    assert_eq!(app.restart_due(), Some(PathBuf::from("/bin/rca-new")));
}

#[test]
fn does_not_restart_when_the_installed_program_is_unknown() {
    let mut app = new_app();
    handle_agent_event(UiEvent::Update(Progress::Ready(None)), &mut app);
    assert_eq!(app.restart_due(), None);
}

#[test]
fn shows_the_restart_and_brings_back_the_unsent_input() {
    let mut app = new_app();
    app.restored_after_update("half a prompt".into());
    let shown = screen(&mut app);
    assert!(
        shown.contains(concat!(
            "restarted after update to v",
            env!("CARGO_PKG_VERSION")
        )),
        "{shown}"
    );
    assert_eq!(app.input, "half a prompt");
    assert_eq!(app.cursor, "half a prompt".len());
}

#[test]
fn does_not_restart_in_loop_mode() {
    let mut app = new_app();
    installed(&mut app);
    app.toggle_loop();
    assert_eq!(app.restart_due(), None);
    app.toggle_loop();
    assert_eq!(app.restart_due(), Some(PathBuf::from("/bin/rca-new")));
}
