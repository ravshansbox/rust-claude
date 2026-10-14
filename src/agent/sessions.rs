use anyhow::{Result, anyhow};
use serde_json::Value;

use super::Agent;
use crate::{
    session::{Session, SessionSummary},
    tools,
};

impl Agent {
    pub async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let id = self.session.id.clone();
        tokio::task::spawn_blocking(move || Session::list_except(&id)).await?
    }

    pub async fn resume(&mut self, id: &str) -> Result<Vec<Value>> {
        let id = id.to_string();
        let (session, messages) = tokio::task::spawn_blocking(move || Session::load(&id)).await??;
        self.session = session;
        self.messages.clone_from(&messages);
        self.background = tools::background::Background::default();
        Ok(messages)
    }

    pub fn new_session(&mut self) -> Result<()> {
        self.session = Session::new()?;
        self.messages.clear();
        self.background = tools::background::Background::default();
        Ok(())
    }

    /// Saves the session after `error` stopped a prompt or compaction. A save
    /// that also fails is reported after `error`, which stays visible.
    pub(super) fn save_after_failure(&mut self, error: anyhow::Error) -> anyhow::Error {
        match self.session.save(&self.messages) {
            Ok(()) => error,
            Err(save_error) => anyhow!("{error:#}; failed to save session: {save_error:#}"),
        }
    }
}
