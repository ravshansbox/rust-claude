use super::{App, draw::conversation};
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
            row: self.conversation_top + row as usize,
            column: column
                .saturating_sub(area.left())
                .min(area.width.saturating_sub(1)),
        }
    }

    pub(super) fn press_mouse(&mut self, column: u16, row: u16) {
        self.selection = self
            .conversation_area
            .contains(Position::new(column, row))
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

    /// Ends a drag and returns the selected text.
    pub(super) fn release_mouse(&mut self, column: u16, row: u16) -> Option<String> {
        self.drag_mouse(column, row);
        let selection = self
            .selection
            .as_mut()
            .filter(|selection| selection.dragging)?;
        selection.dragging = false;
        let Some((start, end)) = selection.bounds() else {
            self.selection = None;
            return None;
        };
        Some(self.selected_text(start, end))
    }

    /// The text from `start` to `end`, each row on its own line without
    /// trailing spaces.
    fn selected_text(&self, start: Point, end: Point) -> String {
        let width = self.conversation_area.width;
        let height = (end.row - start.row + 1).min(u16::MAX as usize) as u16;
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
                    if (first..=last).contains(&x) {
                        text.push_str(symbol);
                    }
                }
                text.trim_end().to_string()
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
        if row < start.row || row > end.row {
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
mod tests {
    use crate::tui::{
        App, Role,
        draw::draw,
        keys::{Action, handle_input},
        test_support::{app_with_reply, screen},
    };
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Modifier};

    fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) -> Vec<String> {
        let mut copied = Vec::new();
        let event = Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
        handle_input(event, app, |action| {
            if let Action::Copy(text) = action {
                copied.push(text);
            }
        });
        copied
    }

    fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) -> Vec<String> {
        let left = MouseButton::Left;
        mouse(app, MouseEventKind::Down(left), from.0, from.1);
        screen(app);
        mouse(app, MouseEventKind::Drag(left), to.0, to.1);
        screen(app);
        mouse(app, MouseEventKind::Up(left), to.0, to.1)
    }

    /// Column and row where `text` starts on the screen.
    fn position(app: &mut App, text: &str) -> (u16, u16) {
        let shown = screen(app);
        shown
            .lines()
            .enumerate()
            .find_map(|(row, line)| {
                line.find(text)
                    .map(|index| (line[..index].chars().count() as u16, row as u16))
            })
            .unwrap_or_else(|| panic!("{text:?} not shown in\n{shown}"))
    }

    fn buffer(app: &mut App) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn reversed_text(app: &mut App) -> String {
        let buffer = buffer(app);
        buffer
            .content()
            .iter()
            .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn copies_the_text_dragged_over_on_release() {
        let mut app = app_with_reply();
        let (column, row) = position(&mut app, "Earlier reply");
        let copied = drag(&mut app, (column, row), (column + 6, row));
        assert_eq!(copied, ["Earlier"]);
    }

    #[test]
    fn copies_a_selection_dragged_backwards() {
        let mut app = app_with_reply();
        let (column, row) = position(&mut app, "Earlier reply");
        let copied = drag(&mut app, (column + 12, row), (column + 8, row));
        assert_eq!(copied, ["reply"]);
    }

    #[test]
    fn copies_each_row_on_its_own_line_without_trailing_spaces() {
        let mut app = app_with_reply();
        app.push(Role::Event, "second message");
        let (column, row) = position(&mut app, "reply");
        let (end_column, end_row) = position(&mut app, "second message");
        let copied = drag(&mut app, (column, row), (end_column + 5, end_row));
        assert_eq!(copied, ["reply\n\nsecond"]);
    }

    #[test]
    fn highlights_the_selection_until_the_next_click() {
        let mut app = app_with_reply();
        let (column, row) = position(&mut app, "Earlier reply");
        let left = MouseButton::Left;
        mouse(&mut app, MouseEventKind::Down(left), column, row);
        assert_eq!(reversed_text(&mut app), "");
        mouse(&mut app, MouseEventKind::Drag(left), column + 6, row);
        assert_eq!(reversed_text(&mut app), "Earlier");
        mouse(&mut app, MouseEventKind::Up(left), column + 6, row);
        assert_eq!(reversed_text(&mut app), "Earlier");
        let copied = mouse(&mut app, MouseEventKind::Down(left), column, row);
        assert!(copied.is_empty());
        mouse(&mut app, MouseEventKind::Up(left), column, row);
        assert_eq!(reversed_text(&mut app), "");
    }

    #[test]
    fn copies_wide_characters_once() {
        let mut app = app_with_reply();
        app.push(Role::Event, "日本語 text");
        let (column, row) = position(&mut app, "text");
        let copied = drag(&mut app, (column - 7, row), (column + 3, row));
        assert_eq!(copied, ["日本語 text"]);
    }

    #[test]
    fn copies_nothing_on_a_click() {
        let mut app = app_with_reply();
        let (column, row) = position(&mut app, "Earlier reply");
        let copied = drag(&mut app, (column, row), (column, row));
        assert!(copied.is_empty());
        assert_eq!(reversed_text(&mut app), "");
    }

    #[test]
    fn keeps_the_selection_on_the_same_text_while_scrolling() {
        let mut app = app_with_reply();
        for number in 0..40 {
            app.push(Role::Event, format!("event {number}"));
        }
        let (column, row) = position(&mut app, "event 35");
        drag(&mut app, (column, row), (column + 7, row));
        mouse(&mut app, MouseEventKind::ScrollUp, 0, 0);
        assert_ne!(position(&mut app, "event 35"), (column, row));
        assert_eq!(reversed_text(&mut app), "event 35");
    }

    #[test]
    fn ignores_drags_that_start_outside_the_conversation() {
        let mut app = app_with_reply();
        let copied = drag(&mut app, (0, 23), (0, 2));
        assert!(copied.is_empty());
        assert_eq!(reversed_text(&mut app), "");
    }

    #[test]
    fn drops_the_selection_when_the_terminal_is_resized() {
        let mut app = app_with_reply();
        let (column, row) = position(&mut app, "Earlier reply");
        drag(&mut app, (column, row), (column + 6, row));
        handle_input(Event::Resize(100, 24), &mut app, |_| {});
        assert_eq!(reversed_text(&mut app), "");
    }
}
