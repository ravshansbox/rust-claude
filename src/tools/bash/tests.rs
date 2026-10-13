use crate::tools::{MAX_OUTPUT, call, test_support::TemporaryFile};
use serde_json::json;
use std::time::{Duration, Instant};

#[tokio::test]
async fn runs_bash_command_without_the_terminal() {
    let command = "ps -o stat= -p $$ | grep -q s && echo own session; \
                   (exec 3</dev/tty) 2>/dev/null || echo no terminal";
    let input = json!({ "command": command, "timeout": 5 });
    assert_eq!(
        call("bash", &input).await,
        Ok("own session\nno terminal\n".into())
    );
}

#[tokio::test]
async fn keeps_output_when_bash_command_times_out() {
    let input = json!({ "command": "echo before; echo problem >&2; sleep 5", "timeout": 1 });
    assert_eq!(
        call("bash", &input).await,
        Err("before\n\nstderr:\nproblem\n\ncommand timed out after 1s".into())
    );
}

#[tokio::test]
async fn keeps_exit_status_when_output_is_truncated() {
    let input =
        json!({ "command": format!("head -c {} /dev/zero | tr '\\0' a; exit 3", MAX_OUTPUT * 2) });
    let text = call("bash", &input).await.unwrap();
    assert!(text.ends_with("exit status: 3"));
}

#[tokio::test]
async fn keeps_start_and_end_of_long_output_from_both_streams() {
    let command = format!(
        "echo first; head -c {} /dev/zero | tr '\\0' a; echo last; echo problem >&2",
        MAX_OUTPUT * 50
    );
    let text = call("bash", &json!({ "command": command })).await.unwrap();
    assert!(text.starts_with("first\n"), "{}", &text[..20]);
    assert!(text.contains("bytes left out"));
    assert!(text.contains("last\n\nstderr:\nproblem\n"));
    assert!(text.len() <= MAX_OUTPUT + 100, "{}", text.len());
}

#[test]
fn cuts_long_output_between_characters() {
    let mut captured = super::Captured::default();
    captured.push("é".repeat(MAX_OUTPUT * 2).as_bytes());
    captured.push(b"xy");
    let text = captured.text(MAX_OUTPUT - 1);
    assert!(!text.contains('\u{FFFD}'));
    assert!(text.starts_with('é') && text.ends_with("éxy"));
}

#[tokio::test]
async fn gives_one_stream_the_room_the_other_does_not_use() {
    let command = format!("head -c {} /dev/zero | tr '\\0' a", MAX_OUTPUT * 3 / 4);
    let text = call("bash", &json!({ "command": command })).await.unwrap();
    assert_eq!(text.len(), MAX_OUTPUT * 3 / 4);
}

#[tokio::test]
async fn reports_exit_status_once() {
    let input = json!({ "command": "echo hi; exit 2" });
    assert_eq!(
        call("bash", &input).await,
        Ok("hi\n\nexit status: 2".into())
    );
}

#[tokio::test]
async fn returns_when_background_process_keeps_output_open() {
    let input = json!({ "command": "sleep 30 & echo $!", "timeout": 10 });
    let started = Instant::now();
    let text = call("bash", &input).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    let killed = std::process::Command::new("kill")
        .arg(text.trim())
        .status()
        .unwrap()
        .success();
    assert!(killed);
}

#[tokio::test]
async fn keeps_output_that_keeps_arriving_after_the_command_exits() {
    let command = "(for i in 1 2 3 4 5 6 7 8; do sleep 0.03; echo $i; done) &";
    let text = call("bash", &json!({ "command": command })).await.unwrap();
    assert_eq!(text, "1\n2\n3\n4\n5\n6\n7\n8\n");
}

#[tokio::test]
async fn kills_background_processes_on_timeout() {
    let pid_file = TemporaryFile::new("pid", "");
    let command = format!("sleep 30 & echo $! > {}; wait", pid_file.path().display());
    let input = json!({ "command": command, "timeout": 1 });
    assert!(call("bash", &input).await.is_err());
    let pid = pid_file.content();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .status()
        .unwrap()
        .success();
    assert!(!alive);
}
