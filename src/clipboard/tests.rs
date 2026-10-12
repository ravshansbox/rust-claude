use super::output;
use std::time::{Duration, Instant};
use tokio::process::Command;

#[tokio::test]
async fn returns_what_a_helper_prints() {
    let printed = output(
        Command::new("sh").args(["-c", "printf image"]),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(printed.status.success());
    assert_eq!(printed.stdout, b"image");
}

#[tokio::test]
async fn stops_a_helper_that_does_not_answer() {
    let started = Instant::now();
    let error = output(Command::new("sleep").arg("30"), Duration::from_millis(200))
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(error.to_string(), "sleep did not answer within 0.2 seconds");
}
