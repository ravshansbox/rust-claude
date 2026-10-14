use std::{sync::Arc, time::Duration};

use super::{Http, OAuthSession, SESSION_HEADER, get};

/// Ends the server's session when the connection is dropped.
pub(in crate::mcp) struct HttpSession(pub(super) Arc<Http>);

impl HttpSession {
    /// Who the server acts for, when it signs in with OAuth.
    pub(in crate::mcp) fn user(&self) -> Option<String> {
        let oauth = self.0.oauth.as_ref()?;
        Some(match oauth.current() {
            None => "anonymous".into(),
            Some(_) => "signed in".into(),
        })
    }

    pub(in crate::mcp) fn take_renewal_notice(&self) -> Option<String> {
        let oauth = self.0.oauth.as_ref()?;
        oauth.renewal_notice.lock().ok()?.take()
    }
}

impl Drop for HttpSession {
    fn drop(&mut self) {
        let session = get(&self.0.session);
        let (Some(session), Ok(runtime)) = (session, tokio::runtime::Handle::try_current()) else {
            return;
        };
        let mut request = self
            .0
            .client
            .delete(&self.0.url)
            .headers(get(&self.0.headers))
            .header(SESSION_HEADER, session)
            .timeout(Duration::from_secs(1));
        if let Some(version) = get(&self.0.protocol_version) {
            request = request.header("mcp-protocol-version", version);
        }
        if let Some(saved) = self.0.oauth.as_ref().and_then(OAuthSession::current) {
            request = request.bearer_auth(saved.access_token);
        }
        runtime.spawn(async move {
            let _ = request.send().await;
        });
    }
}
