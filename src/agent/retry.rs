use std::time::Duration;

pub(super) const MAX_RETRIES: u32 = 3;

const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug)]
struct Retryable {
    message: String,
    retry_after: Option<Duration>,
}

impl std::fmt::Display for Retryable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Retryable {}

pub(super) fn retryable(message: impl ToString, retry_after: Option<Duration>) -> anyhow::Error {
    Retryable {
        message: message.to_string(),
        retry_after,
    }
    .into()
}

pub(super) fn retry_delay(error: &anyhow::Error, attempt: u32) -> Option<Duration> {
    let error = error.downcast_ref::<Retryable>()?;
    if attempt >= MAX_RETRIES {
        return None;
    }
    match error.retry_after {
        Some(delay) if delay > MAX_RETRY_AFTER => None,
        Some(delay) => Some(delay),
        None => Some(Duration::from_secs(1 << attempt)),
    }
}

pub(super) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.as_u16() == 529
        || status.is_server_error()
}

pub(super) fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Duration::from_secs)
}

/// Whether the API rejected a request because a thinking block was made for
/// an earlier system prompt, tool list or history than the one sent now.
pub(super) fn is_thinking_mismatch(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .contains("The block is bound to a different conversation")
}

#[cfg(test)]
mod tests {
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
}
