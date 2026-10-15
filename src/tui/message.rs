mod finished;
mod rendered;

use super::render::render_message;
use finished::finished_markdown_len;
pub(super) use rendered::Rendered;

#[derive(Clone, Copy)]
pub(super) enum Role {
    User,
    Assistant,
    Thinking,
    Tool,
    Event,
}

pub(super) struct ChatMessage {
    pub(super) role: Role,
    pub(super) text: String,
    pub(super) rendered: Option<Rendered>,
    /// Whether text is still streaming in, so the reply may be rendered in
    /// parts that split at blank lines.
    pub(super) streaming: bool,
}

impl ChatMessage {
    pub(super) fn append(&mut self, text: &str) {
        self.text.push_str(text);
        self.streaming = true;
    }

    /// Renders a reply rendered in parts again as a whole, so a link defined
    /// further down also shows in the parts before it.
    pub(super) fn finish_streaming(&mut self) {
        if std::mem::take(&mut self.streaming) {
            self.rendered = None;
        }
    }

    pub(super) fn replace(&mut self, text: String) {
        self.text = text;
        self.rendered = None;
    }

    /// Renders the text if it changed. A streaming reply keeps the lines of
    /// its closed code blocks and what comes before them, since highlighting
    /// code is slow, and renders only what follows. A new width keeps the
    /// lines of messages that look the same at every width.
    pub(super) fn render(&mut self, width: u16) {
        if let Some(rendered) = &mut self.rendered
            && rendered.width != width
            && !depends_on_width(self.role, &self.text)
        {
            rendered.rewrap(width);
        }
        if self
            .rendered
            .as_ref()
            .is_some_and(|rendered| rendered.width == width && rendered.text_len == self.text.len())
        {
            return;
        }
        let mut rendered = match self.rendered.take() {
            Some(rendered) if rendered.width == width => rendered,
            _ => Rendered::new(width),
        };
        rendered.lines.truncate(rendered.finished_lines);
        rendered.heights.truncate(rendered.finished_lines);
        if matches!(self.role, Role::Assistant) && self.streaming {
            let start = rendered.finished_text;
            let finished = start + finished_markdown_len(&self.text[start..], self.streaming);
            if finished > start {
                rendered.extend(render_message(
                    self.role,
                    &self.text[start..finished],
                    width,
                ));
                rendered.finished_text = finished;
                rendered.finished_lines = rendered.lines.len();
            }
        }
        let rest = &self.text[rendered.finished_text..];
        rendered.extend(render_message(self.role, rest, width));
        rendered.text_len = self.text.len();
        self.rendered = Some(rendered);
    }

    /// Rows the message takes once rendered and wrapped.
    pub(super) fn height(&self) -> usize {
        self.rendered.as_ref().map_or(0, |rendered| rendered.height)
    }
}

/// Whether the message's lines change with the width: user messages are
/// padded to it and replies fit their tables to it.
fn depends_on_width(role: Role, text: &str) -> bool {
    match role {
        Role::User | Role::Tool => true,
        Role::Assistant => has_table(text),
        Role::Thinking | Role::Event => false,
    }
}

/// Whether the markdown may hold a table, by looking for a line that could
/// be its delimiter row, such as `| --- | :-: |`, also inside a quote.
fn has_table(text: &str) -> bool {
    text.lines().any(|line| {
        let row = line.trim_start_matches(|c: char| c == '>' || c.is_whitespace());
        row.contains('|')
            && row.contains('-')
            && row
                .chars()
                .all(|c| matches!(c, '|' | '-' | ':') || c.is_whitespace())
    })
}

#[cfg(test)]
mod tests;
