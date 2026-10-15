use super::App;
use crate::tui::message::{ChatMessage, Role};

impl App {
    pub(in crate::tui) fn show_event(&mut self, message: Option<usize>, text: String) -> usize {
        if let Some(message) = message {
            self.messages[message].replace(text);
            message
        } else {
            self.push(Role::Event, text);
            self.messages.len() - 1
        }
    }

    /// Rows left blank above the message: none above the first one or
    /// between notices in a row, one elsewhere.
    pub(in crate::tui) fn gap_before(&self, index: usize) -> usize {
        let notices = index
            .checked_sub(1)
            .and_then(|previous| self.messages.get(previous))
            .zip(self.messages.get(index))
            .is_some_and(|(previous, message)| {
                matches!((previous.role, message.role), (Role::Event, Role::Event))
            });
        usize::from(index > 0 && !notices)
    }

    pub(in crate::tui) fn push(&mut self, role: Role, text: impl Into<String>) {
        self.reads.clear();
        if let Some(last) = self.messages.last_mut() {
            last.finish_streaming();
        }
        self.messages.push(ChatMessage {
            role,
            text: text.into(),
            rendered: None,
            streaming: false,
        });
    }

    pub(in crate::tui) fn set_message_text(&mut self, index: usize, text: String) {
        if let Some(message) = self.messages.get_mut(index) {
            message.replace(text);
        }
    }
}
