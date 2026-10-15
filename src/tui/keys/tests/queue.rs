use super::super::{Action, handle_input};
use crate::images::Image;
use crate::tui::{
    Activity, App, UiEvent,
    replay::handle_agent_event,
    test_support::{new_app, press, press_with},
};
use crossterm::event::{Event, KeyCode, KeyModifiers};

fn queue_while_working(app: &mut App, prompt: &str) {
    app.start(Activity::Working);
    handle_input(Event::Paste(prompt.into()), app, |_| {});
    let mut submitted = false;
    press_with(app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        submitted |= matches!(action, Action::Submit(..));
    });
    assert!(!submitted);
}

fn queued_prompts(app: &App) -> Vec<String> {
    app.queue
        .lock()
        .map(|queue| queue.iter().map(|queued| queued.prompt.clone()).collect())
        .unwrap_or_default()
}

#[test]
fn queues_prompts_while_working_and_sends_them_when_done() {
    let mut app = new_app();
    queue_while_working(&mut app, "first");
    queue_while_working(&mut app, "second");
    assert_eq!(app.input, "");
    assert_eq!(queued_prompts(&app), ["first", "second"]);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    let (prompt, images) = app.send_queued().unwrap();
    assert_eq!(prompt, "first\n\nsecond");
    assert_eq!(images, Vec::<Image>::new());
    assert!(app.busy());
    assert_eq!(queued_prompts(&app), Vec::<String>::new());
    assert_eq!(app.messages.last().unwrap().text, "first\n\nsecond");
}

#[test]
fn does_not_queue_commands() {
    let mut app = new_app();
    app.start(Activity::Working);
    handle_input(Event::Paste("/new".into()), &mut app, |_| {});
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.input, "/new");
    assert_eq!(queued_prompts(&app), Vec::<String>::new());
}

#[test]
fn restores_queued_prompts_when_cancelled() {
    let mut app = new_app();
    queue_while_working(&mut app, "first");
    handle_input(Event::Paste("draft".into()), &mut app, |_| {});
    handle_agent_event(UiEvent::Cancelled(Ok(())), &mut app);
    assert_eq!(app.input, "first\n\ndraft");
    assert_eq!(queued_prompts(&app), Vec::<String>::new());
    assert!(app.send_queued().is_none());
}

#[test]
fn restores_queued_prompts_when_esc_comes_as_the_reply_finishes() {
    let mut app = new_app();
    queue_while_working(&mut app, "first");
    let mut cancelled = false;
    press_with(&mut app, KeyCode::Esc, KeyModifiers::NONE, |action| {
        cancelled |= matches!(action, Action::Cancel);
    });
    assert!(cancelled);
    handle_agent_event(UiEvent::Done(Ok(())), &mut app);
    assert!(!app.busy());
    assert_eq!(app.input, "first");
    assert!(app.send_queued().is_none());
    assert_eq!(app.messages.last().unwrap().text, "cancelled");
}

#[test]
fn restores_queued_prompts_when_the_prompt_fails() {
    let mut app = new_app();
    queue_while_working(&mut app, "first");
    handle_agent_event(UiEvent::Done(Err(anyhow::anyhow!("overloaded"))), &mut app);
    assert!(!app.busy());
    assert_eq!(app.input, "first");
    assert!(app.send_queued().is_none());
    assert_eq!(app.messages.last().unwrap().text, "error: overloaded");
}

#[test]
fn restores_queued_images_ahead_of_the_draft_images() {
    let image = |byte| Image {
        media_type: "image/png",
        data: vec![byte],
    };
    let mut app = new_app();
    app.start(Activity::Working);
    handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(1)))), &mut app);
    press(&mut app, KeyCode::Enter);
    handle_agent_event(UiEvent::ImagePasted(Ok(Some(image(2)))), &mut app);
    handle_agent_event(UiEvent::Done(Err(anyhow::anyhow!("overloaded"))), &mut app);
    assert_eq!(app.input, "[image 1]\n\n[image 2]");
    let mut sent = None;
    press_with(&mut app, KeyCode::Enter, KeyModifiers::NONE, |action| {
        if let Action::Submit(_, images, _) = action {
            sent = Some(images);
        }
    });
    assert_eq!(sent, Some(vec![image(1), image(2)]));
}
