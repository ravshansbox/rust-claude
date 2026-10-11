use std::collections::HashMap;

use super::{
    App, Role,
    status::{format_context, format_stats},
};
use crate::{agent, agent::Stats, tools};

pub(super) struct Task {
    message: usize,
    description: String,
    tool_calls: usize,
    stats: Option<Stats>,
}

/// The subagent tasks of the running `agent` call, by task number.
pub(super) type Tasks = HashMap<usize, Task>;

fn header(task: usize, description: &str) -> String {
    format!("[agent {}] {description}", task + 1)
}

impl Task {
    fn line(&self, task: usize) -> String {
        task_line(
            task,
            &self.description,
            self.tool_calls,
            self.stats.as_ref(),
        )
    }
}

fn task_line(task: usize, description: &str, tool_calls: usize, stats: Option<&Stats>) -> String {
    let mut parts = vec![header(task, description)];
    if tool_calls > 0 {
        parts.push(tools::agent::tool_calls(tool_calls));
    }
    if let Some(stats) = stats {
        parts.extend([format_context(stats), format_stats(stats)]);
    }
    parts.retain(|part| !part.is_empty());
    parts.join(" · ")
}

/// A finished task as the interface shows it, for print mode.
pub(crate) fn finished_task(
    task: usize,
    description: &str,
    tool_calls: usize,
    stats: Option<&Stats>,
    failure: Option<&str>,
) -> String {
    finished(&task_line(task, description, tool_calls, stats), failure)
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
        let task_state = Task {
            message: self.messages.len() - 1,
            description: description.to_string(),
            tool_calls: 0,
            stats: None,
        };
        self.tasks.insert(task, task_state);
    }

    fn update_task(&mut self, task: usize, change: impl FnOnce(&mut Task)) {
        let Some(state) = self.tasks.get_mut(&task) else {
            return;
        };
        change(state);
        let (message, text) = (state.message, state.line(task));
        self.set_message_text(message, text);
    }

    pub(super) fn count_task_tool_calls(&mut self, task: usize, count: usize) {
        self.update_task(task, |state| state.tool_calls = count);
    }

    pub(super) fn show_task_stats(&mut self, task: usize, stats: Stats) {
        self.update_task(task, |state| state.stats = Some(stats));
    }

    pub(super) fn finish_task(&mut self, task: usize, tool_calls: usize, failure: Option<&str>) {
        let Some(mut state) = self.tasks.remove(&task) else {
            return;
        };
        state.tool_calls = tool_calls;
        self.set_message_text(state.message, finished(&state.line(task), failure));
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
