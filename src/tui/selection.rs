use super::{
    App,
    draw::{conversation, saturating_u16},
};
use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::Style,
    text::Span,
    widgets::Widget,
};

/// A cell of the conversation, counting rows from its top.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Point {
    row: usize,
    column: u16,
}

pub(super) struct Selection {
    anchor: Point,
    focus: Point,
    dragging: bool,
}

impl Selection {
    /// The first and last selected cells, or `None` when nothing is selected.
    fn bounds(&self) -> Option<(Point, Point)> {
        (self.anchor != self.focus)
            .then(|| (self.anchor.min(self.focus), self.anchor.max(self.focus)))
    }
}

impl App {
    fn point(&self, column: u16, row: u16) -> Point {
        let area = self.conversation_area;
        let row = row.clamp(area.top(), area.bottom().saturating_sub(1)) - area.top();
        Point {
            row: self.conversation_top + usize::from(row),
            column: column
                .saturating_sub(area.left())
                .min(area.width.saturating_sub(1)),
        }
    }

    pub(super) fn press_mouse(&mut self, column: u16, row: u16) {
        let position = Position::new(column, row);
        self.selection = (self.conversation_area.contains(position)
            && !self.suggestions_area.contains(position))
        .then(|| {
            let point = self.point(column, row);
            Selection {
                anchor: point,
                focus: point,
                dragging: true,
            }
        });
    }

    pub(super) fn drag_mouse(&mut self, column: u16, row: u16) {
        let point = self.point(column, row);
        if let Some(selection) = &mut self.selection
            && selection.dragging
        {
            selection.focus = point;
        }
    }

    /// Ends a drag and quotes the selected text in the input.
    pub(super) fn release_mouse(&mut self, column: u16, row: u16) {
        self.drag_mouse(column, row);
        let Some(selection) = self.selection.take().filter(|selection| selection.dragging) else {
            return;
        };
        if let Some((start, end)) = selection.bounds() {
            let text = self.selected_text(start, end);
            if !text.trim().is_empty() {
                self.quote(&text);
            }
        }
    }

    fn quote(&mut self, text: &str) {
        let before = &self.input[..self.cursor];
        let space = if before.is_empty() || before.ends_with(char::is_whitespace) {
            ""
        } else {
            " "
        };
        let text = crate::highlight::normalise_lines(text);
        self.insert(&format!("{space}'{text}' - "));
    }

    /// The text from `start` to `end`, each row on its own line without
    /// trailing spaces.
    fn selected_text(&self, start: Point, end: Point) -> String {
        let width = self.conversation_area.width;
        let height = saturating_u16(end.row - start.row + 1);
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        conversation(self, width, start.row, height).render(area, &mut buffer);
        (0..height)
            .map(|y| {
                let first = if y == 0 { start.column } else { 0 };
                let last = if y == height - 1 {
                    end.column
                } else {
                    width - 1
                };
                let mut text = String::new();
                let mut covered = 0;
                for x in 0..width {
                    if covered > 0 {
                        covered -= 1;
                        continue;
                    }
                    let symbol = buffer[(x, y)].symbol();
                    covered = Span::raw(symbol).width().saturating_sub(1);
                    if x <= last && x.saturating_add(saturating_u16(covered)) >= first {
                        text.push_str(symbol);
                    }
                }
                text.truncate(text.trim_end().len());
                text
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Shows the selected cells of the conversation in reverse video.
pub(super) fn highlight_selection(app: &App, buffer: &mut Buffer) {
    let Some((start, end)) = app.selection.as_ref().and_then(Selection::bounds) else {
        return;
    };
    let area = app.conversation_area;
    for (y, row) in (area.top()..area.bottom()).zip(app.conversation_top..) {
        if !(start.row..=end.row).contains(&row) {
            continue;
        }
        let first = if row == start.row { start.column } else { 0 };
        let last = if row == end.row {
            end.column
        } else {
            area.width - 1
        };
        let cells = Rect::new(area.left() + first, y, last - first + 1, 1);
        buffer.set_style(cells, Style::new().reversed());
    }
}

#[cfg(test)]
mod tests;
