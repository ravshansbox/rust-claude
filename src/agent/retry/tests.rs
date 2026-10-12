use super::{retry_after, retry_delay, retryable};
use std::time::Duration;

#[test]
fn backs_off_on_retryable_errors() {
    let error = retryable("overloaded", None);
    let delays: Vec<_> = (0..4).map(|attempt| retry_delay(&error, attempt)).collect();
    assert_eq!(
        delays,
        [
            Some(Duration::from_secs(1)),
            Some(Duration::from_secs(2)),
            Some(Duration::from_secs(4)),
            None
        ]
    );
    assert_eq!(retry_delay(&anyhow::anyhow!("bad request"), 0), None);
}

#[test]
fn follows_short_retry_after() {
    let short = retryable("rate limited", Some(Duration::from_secs(10)));
    assert_eq!(retry_delay(&short, 0), Some(Duration::from_secs(10)));
    let long = retryable("rate limited", Some(Duration::from_secs(3600)));
    assert_eq!(retry_delay(&long, 0), None);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
    assert_eq!(retry_after(&headers), Some(Duration::from_secs(7)));
}
