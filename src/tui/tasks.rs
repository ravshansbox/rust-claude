use std::collections::HashMap;

use super::{
    Activity, App, Role,
    status::{format_context, format_stats},
};
use crate::{
    agent::{self, Stats},
    tools,
};

pub(super) struct Task {
    message: usize,
    description: String,
    tool_calls: usize,
    stats: Option<Stats>,
    /// What the subagent shows, as its own conversation.
    view: Box<App>,
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
        self.tasks.insert(
            task,
            Task {
                message: self.messages.len() - 1,
                description: description.to_string(),
                tool_calls: 0,
                stats: None,
                view: Box::default(),
            },
        );
        if let Some(view) = self.task_view(task) {
            view.start(Activity::Working);
        }
    }

    /// The conversation of the subagent on `task`, if it runs.
    pub(super) fn task_view(&mut self, task: usize) -> Option<&mut App> {
        self.tasks.get_mut(&task).map(|state| &mut *state.view)
    }

    /// The running tasks, in order, with their descriptions.
    pub(super) fn task_tabs(&self) -> Vec<(usize, &str)> {
        let mut tabs: Vec<_> = self
            .tasks
            .iter()
            .map(|(task, state)| (*task, state.description.as_str()))
            .collect();
        tabs.sort_unstable();
        tabs
    }

    /// The task line of the running `task`.
    pub(super) fn task_line(&self, task: usize) -> Option<String> {
        self.tasks.get(&task).map(|state| state.line(task))
    }

    pub(super) fn clear_tasks(&mut self) {
        self.tasks.clear();
        self.agent_view = None;
    }

    pub(super) fn open_agent_view(&mut self) {
        self.agent_view = self.task_tabs().first().map(|(task, _)| *task);
    }

    /// Shows the next running agent after the one shown, or the one before
    /// it, wrapping around at the ends.
    pub(super) fn switch_agent(&mut self, forward: bool) {
        let Some(shown) = self.agent_view else {
            return;
        };
        let tasks: Vec<usize> = self.task_tabs().into_iter().map(|(task, _)| task).collect();
        let next = if forward {
            tasks.iter().find(|task| **task > shown).or(tasks.first())
        } else {
            tasks
                .iter()
                .rev()
                .find(|task| **task < shown)
                .or(tasks.last())
        };
        self.agent_view = next.copied();
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
        if self.agent_view == Some(task) {
            self.switch_agent(true);
        }
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
