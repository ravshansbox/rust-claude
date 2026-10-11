use super::{ProcessGroup, argument, bash};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::task::{AbortHandle, JoinSet};

pub const STOP: &str = "stop";
pub const RUN_IN_BACKGROUND: &str = "run_in_background";

pub(super) fn stop_definition() -> Value {
    json!({
        "name": STOP,
        "description": "Stop a command running in the background",
        "input_schema": {
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "The number of the background task" }
            },
            "required": ["id"]
        }
    })
}

pub fn runs_in_background(name: &str, input: &Value) -> bool {
    name == "bash" && input[RUN_IN_BACKGROUND] == true
}

/// Commands running in the background, by task number. Dropping it stops
/// them at once.
#[derive(Default)]
pub struct Background {
    last_id: u64,
    running: HashMap<u64, Running>,
    tasks: JoinSet<(u64, String)>,
}

struct Running {
    command: String,
    task: AbortHandle,
    process_group: ProcessGroup,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Background {
    pub fn start(&mut self, input: &Value) -> Result<String, String> {
        let command = argument(input, "command")?.to_string();
        let (child, timeout) = bash::start(input)?;
        let process_group = ProcessGroup(child.id());
        self.last_id += 1;
        let id = self.last_id;
        let task = self.tasks.spawn(async move {
            match bash::wait(child, timeout).await {
                Ok(output) | Err(output) => (id, output),
            }
        });
        let running = Running {
            command,
            task,
            process_group,
        };
        self.running.insert(id, running);
        Ok(format!(
            "started background task {id}. Its output arrives in a later message when it finishes."
        ))
    }

    pub fn stop(&mut self, input: &Value) -> Result<String, String> {
        let id = input["id"].as_u64().ok_or("missing argument: id")?;
        self.running
            .remove(&id)
            .ok_or_else(|| format!("no background task {id} is running"))?;
        Ok(format!("stopped background task {id}"))
    }

    pub fn is_empty(&self) -> bool {
        self.running.is_empty()
    }

    pub fn len(&self) -> usize {
        self.running.len()
    }

    /// Waits for the next command to finish, and gives the message that
    /// tells the model, or `None` when none is running.
    pub async fn next(&mut self) -> Option<String> {
        while let Some(joined) = self.tasks.join_next().await {
            let Ok((id, output)) = joined else { continue };
            if let Some(mut running) = self.running.remove(&id) {
                running.process_group.0 = None;
                return Some(message(id, &running.command, &output));
            }
        }
        None
    }
}

pub fn message(id: u64, command: &str, output: &str) -> String {
    format!(
        "<background-task id=\"{id}\">\n<command>{command}</command>\n<output>{output}</output>\n</background-task>"
    )
}

/// The task number, command and output of a background task message.
pub fn parse_message(text: &str) -> Option<(&str, &str, &str)> {
    let (id, rest) = text
        .strip_prefix("<background-task id=\"")?
        .split_once("\">\n<command>")?;
    let (command, output) = rest
        .strip_suffix("</output>\n</background-task>")?
        .split_once("</command>\n<output>")?;
    Some((id, command, output))
}

/// How the interface shows a background task message: its summary, and
/// its output if any.
pub fn display(message: &str) -> Option<(String, Option<String>)> {
    let (id, _, output) = parse_message(message)?;
    let output = output.trim_end();
    Some((
        format!("task {id} finished"),
        (!output.is_empty()).then(|| output.to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::{Background, message, parse_message};
    use crate::tools::test_support::TemporaryFile;
    use serde_json::json;

    #[test]
    fn refuses_to_stop_a_task_that_is_not_running() {
        assert_eq!(
            Background::default().stop(&json!({ "id": 7 })),
            Err("no background task 7 is running".into())
        );
    }

    #[tokio::test]
    async fn stops_its_commands_at_once_when_dropped() {
        let pid_file = TemporaryFile::new("background-drop-pid", "");
        let command = format!("echo $$ > {}; exec sleep 30", pid_file.path().display());
        let mut background = Background::default();
        background
            .start(&json!({ "command": command, "run_in_background": true }))
            .unwrap();
        let mut pid = String::new();
        for _ in 0..100 {
            pid = pid_file.content();
            if !pid.trim().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        drop(background);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let state = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&state.stdout);
        assert!(
            state.trim().is_empty() || state.starts_with('Z'),
            "process {pid} still running: {state}"
        );
    }

    #[test]
    fn reads_back_a_background_task_message() {
        let text = message(2, "echo </command>", "a\n</output>\n");
        assert_eq!(
            parse_message(&text),
            Some(("2", "echo </command>", "a\n</output>\n"))
        );
    }
}
