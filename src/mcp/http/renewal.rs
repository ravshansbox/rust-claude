use std::time::Duration;

use serde_json::{Value, json};

use super::{Http, get, set};

const CONNECT_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

impl Http {
    /// Posts a request. Tries `initialize` again after a transient failure,
    /// and starts a new session once when the server says it has expired.
    pub(super) async fn post_request(&self, message: &Value) -> Result<reqwest::Response, String> {
        let mut delays = CONNECT_RETRY_DELAYS.iter();
        let mut renewed = false;
        loop {
            drop(self.renewing.lock().await);
            let error = match self.post(message).await {
                Ok(response) => return Ok(response),
                Err(error) => error,
            };
            if let Some(expired) = error.expired.filter(|_| !renewed) {
                renewed = true;
                self.renew(&expired).await?;
                continue;
            }
            match delays.next() {
                Some(delay) if error.transient && message["method"] == "initialize" => {
                    tokio::time::sleep(*delay).await;
                }
                _ => return Err(error.message),
            }
        }
    }

    async fn renew(&self, expired: &str) -> Result<(), String> {
        let _renewing = self.renewing.lock().await;
        let current = get(&self.session);
        if current.as_deref() != Some(expired) {
            return Ok(());
        }
        let initialize = get(&self.initialize).ok_or("session expired before it started")?;
        let previous = get(&self.protocol_version);
        set(&self.session, None);
        set(&self.protocol_version, None);
        let started = self.start_session(&initialize).await;
        if started.is_err()
            && let Ok(mut session) = self.session.lock()
            && session.is_none()
        {
            *session = current;
            set(&self.protocol_version, previous);
        }
        started
    }

    async fn start_session(&self, initialize: &Value) -> Result<(), String> {
        let failed = |error: String| format!("cannot start a new session: {error}");
        let response = self
            .post(initialize)
            .await
            .map_err(|error| failed(error.message))?;
        let mut answered = false;
        self.read_body(response, "initialize", |reply| {
            if reply["id"] != initialize["id"] {
                return false;
            }
            answered = reply["result"].is_object();
            self.negotiated(reply);
            true
        })
        .await
        .map_err(failed)?;
        if !answered {
            return Err(failed("server did not accept initialize".into()));
        }
        self.post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await
            .map_err(|error| failed(error.message))?;
        Ok(())
    }

    pub(super) fn negotiated(&self, reply: &Value) {
        if let Some(version) = reply["result"]["protocolVersion"].as_str() {
            set(&self.protocol_version, Some(version.to_string()));
        }
    }
}
