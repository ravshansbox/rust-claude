use super::render::{borrowed_line, fence_run, next_fence, render_message, wrapped_height};
use ratatui::text::Line;

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

pub(super) struct Rendered {
    pub(super) width: u16,
    pub(super) lines: Vec<Line<'static>>,
    /// Rows each line takes once wrapped to `width`.
    pub(super) heights: Vec<usize>,
    pub(super) height: usize,
    /// Bytes of text these lines show.
    text_len: usize,
    /// Bytes of text, and lines showing them, that stay the same as a reply
    /// streams in: everything up to its last closed code block.
    finished_text: usize,
    finished_lines: usize,
}

impl Rendered {
    fn new(width: u16) -> Self {
        Self {
            width,
            lines: Vec::new(),
            heights: Vec::new(),
            height: 0,
            text_len: 0,
            finished_text: 0,
            finished_lines: 0,
        }
    }

    /// Adds the lines of the markdown that follows. Separately rendered
    /// blocks need the blank line that markdown puts between blocks.
    fn extend(&mut self, lines: Vec<Line<'static>>) {
        if !lines.is_empty() && !self.lines.is_empty() {
            self.lines.push(Line::default());
            self.heights.push(1);
        }
        for line in lines {
            let height = self.line_height(&line);
            self.lines.push(line);
            self.heights.push(height);
        }
        self.height = self.heights.iter().sum();
    }

    /// Wraps the same lines to a new width.
    fn rewrap(&mut self, width: u16) {
        self.width = width;
        self.heights = self
            .lines
            .iter()
            .map(|line| self.line_height(line))
            .collect();
        self.height = self.heights.iter().sum();
    }

    fn line_height(&self, line: &Line) -> usize {
        wrapped_height(line, self.width)
    }

    /// Each row as shown, with the rows it takes once wrapped, from the line
    /// that holds row `from`. Also gives the rows of the lines left out.
    pub(super) fn rows_from(
        &self,
        from: usize,
    ) -> (usize, Box<dyn Iterator<Item = (Line<'_>, usize)> + '_>) {
        let mut skipped = 0;
        let mut first = 0;
        while let Some(height) = self.heights.get(first)
            && skipped + height <= from
        {
            skipped += height;
            first += 1;
        }
        let lines = self.lines.iter().zip(self.heights.iter().copied());
        let rows = lines
            .skip(first)
            .map(|(line, height)| (borrowed_line(line), height));
        (skipped, Box::new(rows))
    }
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
        if matches!(self.role, Role::Assistant) {
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

/// Bytes of markdown up to the end of its last closed code block whose
/// opening fence starts the line, or up to its last whole line of plain
/// text that starts a block after a blank line. Nothing after it changes how
/// the text before it renders, except a link defined further down, which
/// shows once the reply finishes.
fn finished_markdown_len(text: &str, paragraphs: bool) -> usize {
    let mut finished = 0;
    // The code block `next_fence` sees open, by its simpler rule.
    let mut block = None;
    // The fence character and length of the open code block that starts the line.
    let mut open: Option<(char, usize)> = None;
    // HTML can hold blank lines, so after it only code blocks end a part.
    let mut html = false;
    // Fences inside an HTML block, which runs to a blank line, are its text.
    let mut in_html = false;
    let mut after_blank = false;
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        let start = end;
        end += line.len();
        let trimmed = line.trim_start();
        let in_fence = block.is_some();
        block = next_fence(block, line);
        if in_fence || open.is_some() {
            after_blank = false;
        } else {
            let starts_html = trimmed.starts_with('<') && line.len() - trimmed.len() <= 3;
            html |= starts_html;
            in_html = (in_html || starts_html) && !trimmed.is_empty();
            if paragraphs
                && after_blank
                && !html
                && line.ends_with('\n')
                && starts_plain_block(line)
            {
                finished = start;
            }
            after_blank = trimmed.is_empty();
        }
        let Some((fence, length)) = fence_run(trimmed) else {
            continue;
        };
        if length < 3 || in_html {
            continue;
        }
        let indent = line.len() - trimmed.len();
        match open {
            None if indent == 0 && !(fence == '`' && trimmed[length..].contains('`')) => {
                open = Some((fence, length));
            }
            Some((open_fence, open_length))
                if indent <= 3
                    && fence == open_fence
                    && length >= open_length
                    && trimmed[length..].trim().is_empty() =>
            {
                open = None;
                if block.is_none() && line.ends_with('\n') {
                    finished = end;
                }
            }
            _ => {}
        }
    }
    finished
}

/// Whether the line starts a block of its own at the left edge: not
/// indented, not a list item and not HTML.
fn starts_plain_block(line: &str) -> bool {
    let Some(first) = line.chars().next() else {
        return false;
    };
    let marker_end = |rest: &str| rest.is_empty() || rest.starts_with([' ', '\t', '\n']);
    if first.is_whitespace() || first == '<' {
        return false;
    }
    if matches!(first, '-' | '*' | '+') && marker_end(&line[1..]) {
        return false;
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    !(digits > 0 && line[digits..].starts_with(['.', ')']) && marker_end(&line[digits + 1..]))
}

#[cfg(test)]
mod tests;
