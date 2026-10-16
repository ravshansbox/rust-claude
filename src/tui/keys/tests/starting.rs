use super::*;
use crate::tui::keys::submit_starting_prompt;

#[test]
fn sends_the_starting_prompt_with_its_images() {
    let image = |byte| Image {
        media_type: "image/png",
        data: vec![byte],
    };
    let mut app = new_app();
    let mut sent = None;
    submit_starting_prompt(
        &mut app,
        "fix the bug",
        vec![image(1), image(2)],
        |action| {
            if let Action::Submit(prompt, images, _) = action {
                sent = Some((prompt, images));
            }
        },
    );
    assert_eq!(
        sent,
        Some((
            "fix the bug [image 1] [image 2]".into(),
            vec![image(1), image(2)]
        ))
    );
    assert!(app.busy());
    assert!(screen(&mut app).contains("fix the bug [image 1] [image 2]"));
    assert_eq!(app.input, "");
}

#[test]
fn runs_a_starting_prompt_that_is_a_command() {
    let mut app = new_app();
    let mut acted = false;
    let quit = submit_starting_prompt(&mut app, "/effort high", Vec::new(), |_| acted = true);
    assert!(!quit && !acted && !app.busy());
    assert_eq!(app.effort, "high");
    assert!(submit_starting_prompt(
        &mut app,
        "/quit",
        Vec::new(),
        |_| {}
    ));
}
