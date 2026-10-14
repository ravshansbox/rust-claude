use super::super::finish_renewal;
use super::*;

#[tokio::test]
async fn finishes_renewing_after_the_caller_gives_up() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", true).await;
    let path = no_credentials_file();
    let prompt = renew_in_background();
    endpoint.arrived.notified().await;
    prompt.abort();
    let _ = prompt.await;
    endpoint.release.notify_one();
    let mut saved = None;
    for _ in 0..100 {
        saved = std::fs::read_to_string(&path).ok();
        if saved.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let _ = std::fs::remove_file(&path);
    let saved: Credentials = serde_json::from_str(&saved.expect("auth.json not saved")).unwrap();
    assert_eq!(
        (saved.access.as_str(), saved.refresh.as_str()),
        ("access from first", "first+")
    );
}

#[tokio::test]
async fn quitting_waits_for_a_renewal_to_be_saved() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", true).await;
    let path = no_credentials_file();
    let prompt = renew_in_background();
    endpoint.arrived.notified().await;
    prompt.abort();
    let _ = prompt.await;
    let release = endpoint.release.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        release.notify_one();
    });
    finish_renewal(Duration::from_secs(5)).await;
    let saved = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    let saved: Credentials = serde_json::from_str(&saved.expect("auth.json not saved")).unwrap();
    assert_eq!(saved.refresh, "first+");
}

#[tokio::test]
async fn quitting_stops_waiting_for_a_renewal_that_hangs() {
    let _lock = AUTH_FILE.lock().await;
    let endpoint = TokenEndpoint::start("first", true).await;
    let path = no_credentials_file();
    let prompt = renew_in_background();
    endpoint.arrived.notified().await;
    let waited = tokio::time::timeout(
        Duration::from_secs(5),
        finish_renewal(Duration::from_millis(100)),
    )
    .await;
    endpoint.release.notify_one();
    let _ = prompt.await;
    let _ = std::fs::remove_file(&path);
    assert!(waited.is_ok(), "kept waiting for the renewal");
}
