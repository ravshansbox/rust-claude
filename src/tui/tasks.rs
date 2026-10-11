use std::collections::HashMap;

use super::{App, Role};
use crate::{agent, tools};

/// The message and description of each subagent task of the running `agent`
/// call, by task number.
pub(super) type Tasks = HashMap<usize, (usize, String)>;

fn header(task: usize, description: &str) -> String {
    format!("[agent {}] {description}", task + 1)
}

fn header_with_tool_calls(task: usize, description: &str, count: usize) -> String {
    match count {
        0 => header(task, description),
        count => format!(
            "{} · {}",
            header(task, description),
            tools::agent::tool_calls(count)
        ),
    }
}

/// The task, with the first line of why it failed or did not run.
fn finished(header: &str, failure: Option<&str>) -> String {
    match failure.and_then(|failure| failure.lines().next()) {
        Some(line) => format!("{header}\n  {line}"),
        None => header.to_string(),
    }
}

impl App {
    pub(super) fn start_task(&mut self, task: usize, description: &str) {
        self.push(Role::Tool, header(task, description));
        self.tasks
            .insert(task, (self.messages.len() - 1, description.to_string()));
    }

    pub(super) fn count_task_tool_calls(&mut self, task: usize, count: usize) {
        let Some((message, description)) = self.tasks.get(&task) else {
            return;
        };
        let message = *message;
        let text = header_with_tool_calls(task, description, count);
        self.set_message_text(message, text);
    }

    pub(super) fn finish_task(&mut self, task: usize, tool_calls: usize, failure: Option<&str>) {
        let Some((message, description)) = self.tasks.remove(&task) else {
            return;
        };
        let text = finished(
            &header_with_tool_calls(task, &description, tool_calls),
            failure,
        );
        self.set_message_text(message, text);
    }

    /// Shows the tasks of a replayed `agent` call, and why any failed.
    pub(super) fn push_finished_tasks<'a>(
        &mut self,
        tasks: impl Iterator<Item = (&'a str, &'a str)>,
    ) {
        for (task, (description, reply)) in tasks.enumerate() {
            let failure = agent::is_failure(reply).then_some(reply);
            self.push(Role::Tool, finished(&header(task, description), failure));
        }
    }
}
