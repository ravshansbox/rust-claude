use super::{ProcessGroup, argument, bash, tool};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::task::{AbortHandle, JoinSet};

pub const STOP: &str = "bash_stop";
pub const RUN_IN_BACKGROUND: &str = "run_in_background";

pub(super) fn stop_definition() -> Value {
    tool(
        STOP,
        "Stop a command running in the background",
        json!({
            "id": { "type": "integer", "description": "The number of the background task" }
        }),
        &["id"],
    )
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
    pub fn call(&mut self, name: &str, input: &Value) -> Option<Result<String, String>> {
        if runs_in_background(name, input) {
            return Some(self.start(input));
        }
        (name == STOP).then(|| self.stop(input))
    }

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
        self.running.insert(
            id,
            Running {
                command,
                task,
                process_group,
            },
        );
        Ok(format!(
            "started background task {id}. Its output arrives in a later message when it finishes."
        ))
    }

    pub fn stop(&mut self, input: &Value) -> Result<String, String> {
        let id = input["id"].as_u64().ok_or("missing argument: id")?;
        if self
            .running
            .get(&id)
            .is_some_and(|running| running.task.is_finished())
        {
            return Ok(format!(
                "background task {id} has already finished. Its output arrives in a later message."
            ));
        }
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
mod tests;
