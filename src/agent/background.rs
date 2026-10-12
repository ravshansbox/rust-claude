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
mod tests {
    use super::super::test_support::{self, MockApi, text_reply, tool_reply};
    use crate::tools::test_support::{TemporaryFile, is_running};
    use serde_json::{Value, json};
    use std::time::Duration;

    fn tool_result(request: &Value) -> String {
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .rfind(|block| block["type"] == "tool_result")
            .unwrap()["content"]
            .to_string()
    }

    #[tokio::test]
    async fn runs_a_command_in_the_background_and_gives_its_output_when_it_finishes() {
        let gate = TemporaryFile::new("background-gate", "");
        std::fs::remove_file(gate.path()).unwrap();
        let command = format!(
            "while [ ! -e {} ]; do sleep 0.05; done; echo finished",
            gate.path().display()
        );
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "bash",
                json!({ "command": command, "run_in_background": true }),
            ),
            text_reply("waiting"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut counts = Vec::new();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            agent.prompt("watch it", &[], |event| {
                if let super::super::AgentEvent::Stats(stats) = event {
                    counts.push(stats.background);
                }
            }),
        )
        .await;
        let running = agent.has_background();
        std::fs::write(gate.path(), "").unwrap();
        let message = tokio::time::timeout(Duration::from_secs(5), agent.next_background()).await;
        test_support::remove_session(&agent);
        result.expect("prompt waited for the command").unwrap();
        assert!(running);
        assert_eq!(counts, [0, 0, 1, 1]);
        let requests = api.requests().await;
        let result = tool_result(&requests[1]);
        assert!(result.contains("started background task 1"), "{result}");
        let message = message.unwrap().unwrap();
        assert!(message.contains("finished"), "{message}");
        assert!(message.contains(&command), "{message}");
        assert!(!agent.has_background());
    }

    #[tokio::test]
    async fn stops_a_background_command() {
        let pid_file = TemporaryFile::new("background-pid", "");
        let command = format!("echo $$ > {}; exec sleep 30", pid_file.path().display());
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "bash",
                json!({ "command": command, "run_in_background": true }),
            ),
            text_reply("started"),
            tool_reply("call-2", "bash_stop", json!({ "id": 1 })),
            text_reply("stopped"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let started = agent.prompt("start it", &[], |_| {}).await;
        let pid = pid_file.wait_for_content().await;
        let stopped = agent.prompt("stop it", &[], |_| {}).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        test_support::remove_session(&agent);
        started.unwrap();
        stopped.unwrap();
        let requests = api.requests().await;
        let result = tool_result(&requests[3]);
        assert!(result.contains("stopped background task 1"), "{result}");
        assert!(!agent.has_background());
        assert!(!is_running(&pid), "process {pid} still running");
    }

    #[tokio::test]
    async fn stops_background_commands_when_a_new_session_starts() {
        let pid_file = TemporaryFile::new("background-new-session-pid", "");
        let command = format!("echo $$ > {}; exec sleep 30", pid_file.path().display());
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "bash",
                json!({ "command": command, "run_in_background": true }),
            ),
            text_reply("started"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let started = agent.prompt("start it", &[], |_| {}).await;
        let pid = pid_file.wait_for_content().await;
        test_support::remove_session(&agent);
        let new_session = agent.new_session();
        tokio::time::sleep(Duration::from_millis(200)).await;
        started.unwrap();
        new_session.unwrap();
        assert!(!agent.has_background());
        assert!(!is_running(&pid), "process {pid} still running");
    }
}
