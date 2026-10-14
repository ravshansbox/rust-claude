use super::App;
use crate::{
    tools,
    tui::{message::Role, render::tool_message},
};

impl App {
    pub(in crate::tui) fn push_tool(&mut self, name: &str, summary: String, diff: Option<&str>) {
        if name != "read" || diff.is_some() {
            self.push(Role::Tool, tool_message(name, &summary, diff));
            return;
        }
        let merge = !self.reads.is_empty();
        let mut reads = std::mem::take(&mut self.reads);
        reads.add(summary);
        let text = tool_message(name, &reads.summary(), None);
        match self.messages.last_mut() {
            Some(last) if merge => last.replace(text),
            _ => self.push(Role::Tool, text),
        }
        self.reads = reads;
    }

    pub(in crate::tui) fn push_shell(&mut self, command: &str, output: &str) {
        let output = output.trim_end();
        let diff = (!output.is_empty()).then_some(output);
        self.push(Role::Tool, tool_message("!", command, diff));
    }

    pub(in crate::tui) fn push_background(&mut self, message: &str) -> bool {
        let Some((summary, output)) = tools::background::display(message) else {
            return false;
        };
        self.push(
            Role::Tool,
            tool_message("background", &summary, output.as_deref()),
        );
        true
    }
}
