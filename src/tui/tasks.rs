use std::collections::HashMap;

use super::{App, Role};
use crate::agent;

/// The message of each subagent task of the running `agent` call, by task number.
pub(super) type Tasks = HashMap<usize, usize>;

fn header(task: usize, description: &str) -> String {
    format!("[agent {}] {description}", task + 1)
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
        self.tasks.insert(task, self.messages.len() - 1);
    }

    pub(super) fn finish_task(&mut self, task: usize, failure: Option<&str>) {
        let Some(message) = self.tasks.remove(&task) else {
            return;
        };
        let text = finished(&self.messages[message].text, failure);
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
