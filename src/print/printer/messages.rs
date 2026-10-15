use super::Printer;
use crate::{highlight, tui};
use std::io::Write;

impl<Out: Write, Err: Write> Printer<Out, Err> {
    /// Writes a tool call or task as the interface shows it, or as plain
    /// text when `err` is not a terminal.
    pub(in crate::print) fn tool(&mut self, text: &str) {
        self.begin_err();
        if !self.colour {
            let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
            self.line(first.trim_end());
            for line in rest.lines() {
                self.line(line);
            }
            return;
        }
        let columns = crossterm::terminal::size().ok().map(|(columns, _)| columns);
        for line in tui::ansi_tool_lines(text, self.dark, self.background, columns) {
            let _ = writeln!(self.err, "{line}");
        }
    }

    pub(super) fn message(&mut self, text: &str) {
        self.begin_err();
        self.line(text);
    }

    fn line(&mut self, text: &str) {
        let _ = writeln!(self.err, "{}", highlight::strip_controls(text));
    }
}
