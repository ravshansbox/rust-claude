use serde_json::Value;

use super::{Http, MAX_ERROR_BODY, SESSION_HEADER, get, set};

pub(super) struct PostError {
    pub(super) message: String,
    /// A network failure, or a status that may clear up on its own.
    pub(super) transient: bool,
    /// The session the server no longer knows.
    pub(super) expired: Option<String>,
}

impl PostError {
    pub(super) fn fatal(message: String) -> Self {
        Self {
            message,
            transient: false,
            expired: None,
        }
    }
}

impl Http {
    /// Runs the `headersHelper`, if any, and sends its headers from now on.
    async fn run_headers_helper(&self) -> Result<(), PostError> {
        let Some(helper) = &self.headers_helper else {
            return Ok(());
        };
        let dynamic = helper.run(&self.url).await.map_err(PostError::fatal)?;
        let mut headers = self.static_headers.clone();
        for (name, value) in &dynamic {
            headers.insert(name, value.clone());
        }
        set(&self.headers, headers);
        Ok(())
    }

    pub(super) async fn post(&self, message: &Value) -> Result<reqwest::Response, PostError> {
        let initialize = message["method"] == "initialize";
        if initialize {
            self.run_headers_helper().await?;
        }
        let mut rejected = None;
        let mut helper_ran_again = initialize;
        loop {
            let token = self.access_token(rejected.as_deref()).await?;
            let (response, session) = self.post_once(message, token.as_deref()).await?;
            let Some(oauth) = &self.oauth else {
                if response.status().as_u16() == 401
                    && self.headers_helper.is_some()
                    && !helper_ran_again
                {
                    helper_ran_again = true;
                    self.run_headers_helper().await?;
                    continue;
                }
                return self.check(message, response, session).await;
            };
            let status = response.status().as_u16();
            let challenge = response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default();
            let unauthorised =
                status == 401 || (status == 403 && challenge.contains("insufficient_scope"));
            if !unauthorised {
                return self.check(message, response, session).await;
            }
            if rejected.is_none() && token.is_some() {
                rejected = token;
                continue;
            }
            return Err(oauth.needs_sign_in(None));
        }
    }

    async fn post_once(
        &self,
        message: &Value,
        token: Option<&str>,
    ) -> Result<(reqwest::Response, Option<String>), PostError> {
        // Each post, with the reply read from it, takes at most the server's
        // timeout, so a notification the server never answers does not hold
        // up the messages queued after it. Requests are bounded as a whole
        // by the same timeout, started before this.
        let mut request = self
            .client
            .post(&self.url)
            .headers(get(&self.headers))
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(message)
            .timeout(self.timeout);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let session = get(&self.session);
        if let Some(session) = &session {
            request = request.header(SESSION_HEADER, session);
        }
        if let Some(version) = get(&self.protocol_version) {
            request = request.header("mcp-protocol-version", version);
        }
        let response = request.send().await.map_err(|error| PostError {
            message: format!(
                "cannot reach {}: {:#}",
                self.url,
                anyhow::Error::from(error)
            ),
            transient: true,
            expired: None,
        })?;
        Ok((response, session))
    }

    /// Keeps the session the server gave, and turns error statuses into
    /// errors. `session` is the one the request was sent with.
    async fn check(
        &self,
        message: &Value,
        response: reqwest::Response,
        session: Option<String>,
    ) -> Result<reqwest::Response, PostError> {
        let method = message["method"].as_str().unwrap_or("a reply");
        if let Some(session) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|session| session.to_str().ok())
        {
            set(&self.session, Some(session.to_string()));
        }
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let body = response.text().await.unwrap_or_default();
        let body: String = body.trim().chars().take(MAX_ERROR_BODY).collect();
        let status = status.as_u16();
        let message = if body.is_empty() {
            format!("server answered {method} with status {status}")
        } else {
            format!("server answered {method} with status {status}: {body}")
        };
        Err(PostError {
            message,
            transient: status == 408 || status == 429 || (status >= 500 && status != 501),
            expired: session.filter(|_| status == 404),
        })
    }
}
