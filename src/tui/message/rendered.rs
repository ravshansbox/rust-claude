use crate::tui::render::{borrowed_line, wrapped_height};
use ratatui::text::Line;

pub(in crate::tui) struct Rendered {
    pub(in crate::tui) width: u16,
    pub(in crate::tui) lines: Vec<Line<'static>>,
    /// Rows each line takes once wrapped to `width`.
    pub(in crate::tui) heights: Vec<usize>,
    pub(in crate::tui) height: usize,
    /// Bytes of text these lines show.
    pub(super) text_len: usize,
    /// Bytes of text, and lines showing them, that stay the same as a reply
    /// streams in: everything up to its last closed code block.
    pub(super) finished_text: usize,
    pub(super) finished_lines: usize,
}

impl Rendered {
    pub(super) fn new(width: u16) -> Self {
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
    pub(super) fn extend(&mut self, lines: Vec<Line<'static>>) {
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
    pub(super) fn rewrap(&mut self, width: u16) {
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
    pub(in crate::tui) fn rows_from(
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
