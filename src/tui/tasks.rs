use std::collections::HashMap;

use super::{App, Role};
use crate::agent::AgentEvent;

/// Lines of a working subagent shown below its task.
const LIVE_LINES: usize = 5;
/// Lines of a subagent's final reply shown below its task.
const REPLY_LINES: usize = 10;

/// The subagent tasks of the running `agent` call, by task number.
pub(super) type Tasks = HashMap<usize, TaskView>;

pub(super) struct TaskView {
    message: usize,
    header: String,
    lines: Vec<String>,
    /// Whether the last line is text that may go on.
    open: bool,
}

impl TaskView {
    fn add(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Text(text) => self.add_text(text),
            AgentEvent::ToolStart {
                name,
                summary,
                diff,
            } => {
                let detail = match (summary.as_str(), diff) {
                    ("", Some(diff)) => diff.lines().next().unwrap_or_default().trim(),
                    _ => summary,
                };
                self.add_line(format!("{name} {detail}").trim_end());
            }
            AgentEvent::ToolDone {
                name,
                error: Some(error),
                ..
            } => self.add_line(&format!("{name} failed: {}", first_line(error))),
            AgentEvent::Notice(text) => self.add_line(first_line(text)),
            _ => {}
        }
    }

    fn add_text(&mut self, text: &str) {
        for (index, piece) in text.split('\n').enumerate() {
            if index > 0 {
                self.open = false;
            }
            match self.lines.last_mut() {
                Some(last) if self.open => last.push_str(piece),
                _ if piece.trim().is_empty() => {}
                _ => {
                    self.lines.push(piece.to_string());
                    self.open = true;
                }
            }
        }
    }

    fn add_line(&mut self, line: &str) {
        self.lines.push(line.to_string());
        self.open = false;
    }

    fn text(&self) -> String {
        let start = self.lines.len().saturating_sub(LIVE_LINES);
        with_body(&self.header, self.lines[start..].iter().map(String::as_str))
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn header(task: usize, description: &str) -> String {
    format!("[{}] {description}", task + 1)
}

fn with_body<'a>(header: &str, lines: impl Iterator<Item = &'a str>) -> String {
    let mut text = header.to_string();
    for line in lines {
        text.push('\n');
        if !line.is_empty() {
            text.push_str("  ");
            text.push_str(line);
        }
    }
    text
}

/// The task with the start of its final reply, or why it failed.
fn finished(header: &str, reply: &str) -> String {
    let lines: Vec<&str> = reply.trim_end().lines().collect();
    let mut text = with_body(header, lines.iter().take(REPLY_LINES).copied());
    if lines.len() > REPLY_LINES {
        text.push_str("\n  …");
    }
    text
}

impl App {
    pub(super) fn start_task(&mut self, task: usize, description: &str) {
        let header = header(task, description);
        self.push(Role::Tool, header.clone());
        let view = TaskView {
            message: self.messages.len() - 1,
            header,
            lines: Vec::new(),
            open: false,
        };
        self.tasks.insert(task, view);
    }

    pub(super) fn task_event(&mut self, task: usize, event: &AgentEvent) {
        let Some(view) = self.tasks.get_mut(&task) else {
            return;
        };
        view.add(event);
        let (message, text) = (view.message, view.text());
        self.set_message_text(message, text);
    }

    pub(super) fn finish_task(&mut self, task: usize, reply: &str) {
        let Some(view) = self.tasks.remove(&task) else {
            return;
        };
        self.set_message_text(view.message, finished(&view.header, reply));
    }

    /// Shows the tasks of a replayed `agent` call with their final replies.
    pub(super) fn push_finished_tasks<'a>(
        &mut self,
        tasks: impl Iterator<Item = (&'a str, &'a str)>,
    ) {
        for (task, (description, reply)) in tasks.enumerate() {
            self.push(Role::Tool, finished(&header(task, description), reply));
        }
    }
}
