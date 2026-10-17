use super::App;
use crate::tui::{message::Role, render::tool_message};

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

    pub(in crate::tui) fn push_shell(&mut self, command: &str, share: bool, output: &str) {
        let output = output.trim_end();
        let diff = (!output.is_empty()).then_some(output);
        let prefix = if share { "!" } else { "!!" };
        self.push(Role::Tool, tool_message(prefix, command, diff));
    }
}
