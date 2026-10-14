use std::time::Duration;

use anyhow::Result;

/// Gives up on a connection attempt after this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Gives up when no data arrives for this long, so a dead connection fails
/// and gets retried instead of hanging. Long, because the API can take a
/// while to start replying to a large prompt.
const READ_TIMEOUT: Duration = Duration::from_secs(300);

pub fn http_client() -> Result<reqwest::Client> {
    http_client_with(READ_TIMEOUT)
}

pub(super) fn http_client_with(read_timeout: Duration) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(read_timeout)
        .build()?)
}
