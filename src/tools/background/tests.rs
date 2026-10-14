use super::{Background, message, parse_message};
use serde_json::json;
use std::time::Duration;

#[test]
fn refuses_to_stop_a_task_that_is_not_running() {
    assert_eq!(
        Background::default().stop(&json!({ "id": 7 })),
        Err("no background task 7 is running".into())
    );
}

#[test]
fn explains_why_a_task_number_is_not_valid() {
    let cases = [
        (json!({}), "missing argument: id"),
        (json!({ "id": "3" }), "id must be a whole number"),
        (json!({ "id": 1.5 }), "id must be a whole number"),
        (json!({ "id": -1 }), "id must be at least 1"),
    ];
    for (input, error) in cases {
        assert_eq!(
            Background::default().stop(&input),
            Err(error.into()),
            "{input}"
        );
    }
}

#[tokio::test]
async fn keeps_the_output_of_a_task_that_finished_before_it_was_stopped() {
    let mut background = Background::default();
    background
        .start(&json!({ "command": "echo done", "run_in_background": true }))
        .unwrap();
    while !background.running[&1].task.is_finished() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        background.stop(&json!({ "id": 1 })),
        Ok("background task 1 has already finished. Its output arrives in a later message.".into())
    );
    assert_eq!(
        background.next().await,
        Some(message(1, "echo done", "done\n"))
    );
}

#[test]
fn reads_back_a_background_task_message() {
    let text = message(2, "echo </command>", "a\n</output>\n");
    assert_eq!(
        parse_message(&text),
        Some(("2", "echo </command>", "a\n</output>\n"))
    );
}
