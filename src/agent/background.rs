use super::Agent;
use crate::tools::background;
use serde_json::Value;

impl Agent {
    /// Runs a call that starts or stops a background command, or gives
    /// `None` for any other call.
    pub(super) fn call_background(
        &mut self,
        name: &str,
        input: &Value,
    ) -> Option<Result<String, String>> {
        if self.subagent {
            return None;
        }
        if background::runs_in_background(name, input) {
            return Some(self.background.start(input));
        }
        (name == background::STOP).then(|| self.background.stop(input))
    }

    pub fn has_background(&self) -> bool {
        !self.background.is_empty()
    }

    /// Waits for the next background command to finish, and gives the
    /// message that tells the model, or `None` when none is running.
    pub async fn next_background(&mut self) -> Option<String> {
        self.background.next().await
    }
}

#[cfg(test)]
mod tests;
