use crate::tui::{
    Activity, UiEvent, keys::handle_input, replay::handle_agent_event, test_support::new_app,
};
use crossterm::event::Event;

#[test]
fn notifies_when_a_run_ends_while_the_terminal_is_unfocused() {
    let mut app = new_app();
    handle_input(Event::FocusLost, &mut app, |_| {});
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert_eq!(app.notification.take().as_deref(), Some("Done"));
    handle_agent_event(
        UiEvent::Done(Err(anyhow::anyhow!("overloaded\nretry later"))),
        &mut app,
    );
    assert_eq!(
        app.notification.take().as_deref(),
        Some("Failed: overloaded")
    );
}

#[test]
fn does_not_notify_when_focused_cancelled_or_turned_off() {
    let mut focused = new_app();
    handle_input(Event::FocusLost, &mut focused, |_| {});
    handle_input(Event::FocusGained, &mut focused, |_| {});
    let mut cancelled = new_app();
    handle_input(Event::FocusLost, &mut cancelled, |_| {});
    cancelled.start(Activity::Cancelling);
    let mut off = new_app();
    off.notifications = false;
    handle_input(Event::FocusLost, &mut off, |_| {});
    for mut app in [focused, cancelled, off] {
        handle_agent_event(UiEvent::Done(Ok(())), &mut app);
        assert_eq!(app.notification, None);
    }
}
