use super::*;

#[test]
fn quits_with_esc_ctrl_c_ctrl_d_or_the_quit_command() {
    for (code, modifiers) in [
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Char('c'), KeyModifiers::CONTROL),
        (KeyCode::Char('d'), KeyModifiers::CONTROL),
    ] {
        let mut app = new_app();
        assert!(press_with(&mut app, code, modifiers, |_| {}), "{code:?}");
    }
    let mut app = new_app();
    handle_input(Event::Paste("/quit".into()), &mut app, |_| {});
    assert!(press_with(
        &mut app,
        KeyCode::Enter,
        KeyModifiers::NONE,
        |_| {}
    ));
}

#[test]
fn clears_a_draft_instead_of_quitting_on_ctrl_c_or_ctrl_d() {
    let mut app = new_app();
    handle_input(Event::Paste("draft".into()), &mut app, |_| {});
    assert!(!press_with(
        &mut app,
        KeyCode::Char('d'),
        KeyModifiers::CONTROL,
        |_| {}
    ));
    assert_eq!(app.input, "draft");
    assert!(!press_with(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        |_| {}
    ));
    assert_eq!(app.input, "");
}

#[test]
fn ctrl_c_closes_pickers_and_history_search_keeping_the_draft() {
    let mut app = new_app();
    app.add_prompt("fix tests".into());
    handle_input(Event::Paste("draft".into()), &mut app, |_| {});
    press_with(&mut app, KeyCode::Char('r'), KeyModifiers::CONTROL, |_| {});
    handle_input(Event::Paste("fix".into()), &mut app, |_| {});
    assert!(!press_with(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        |_| {}
    ));
    assert!(!screen(&mut app).contains("Prompt history"));
    assert_eq!(app.input, "draft");
    press_with(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL, |_| {});
    handle_input(Event::Paste("/model".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("Select model"));
    assert!(!press_with(
        &mut app,
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        |_| {}
    ));
    assert!(!screen(&mut app).contains("Select model"));
    assert_eq!(app.model, crate::models::DEFAULT);
}
