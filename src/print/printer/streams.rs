use super::Printer;
use crate::{highlight, tui};
use std::io::Write;

impl<Out: Write, Err: Write> Printer<Out, Err> {
    pub(super) fn answer_text(&mut self, text: &str) {
        self.flush_reads();
        self.begin_answer();
        if self.out_terminal {
            let _ = write!(self.out, "{}", highlight::strip_controls(text));
        } else {
            let _ = write!(self.out, "{text}");
        }
        let _ = self.out.flush();
        self.printed = true;
        self.out_open = !text.ends_with('\n');
    }

    pub(super) fn thinking_text(&mut self, text: &str) {
        self.flush_reads();
        if !self.thinking {
            self.begin_err();
            self.thinking = true;
        }
        let shown = text.trim_end_matches('\n');
        if shown.is_empty() {
            self.held_newlines += text.len();
            return;
        }
        let held = text.len() - shown.len();
        let text = "\n".repeat(std::mem::replace(&mut self.held_newlines, held)) + shown;
        if self.colour {
            let _ = write!(self.err, "{}", tui::ansi_thinking(&text));
        } else {
            let _ = write!(self.err, "{}", highlight::strip_controls(&text));
        }
        let _ = self.err.flush();
        self.err_open = true;
    }
}
