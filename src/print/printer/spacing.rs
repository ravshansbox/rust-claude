use super::Printer;
use std::io::Write;

impl<Out: Write, Err: Write> Printer<Out, Err> {
    /// Whether `out` and `err` show on the same terminal, so they share
    /// the blank lines between messages.
    fn shared_screen(&self) -> bool {
        self.out_terminal && self.colour
    }

    /// Ends a line left open on `err`, or on `out` when both may show in the
    /// same place, with a newline on `err`. Only one of them being a
    /// terminal means they show apart.
    fn close_open_line(&mut self) {
        let out_open = std::mem::take(&mut self.out_open) && self.out_terminal == self.colour;
        if std::mem::take(&mut self.err_open) | out_open {
            let _ = writeln!(self.err);
        }
    }

    /// Starts a message on `err` on its own line, after a blank line.
    pub(super) fn begin_err(&mut self) {
        self.close_open_line();
        if self.err_shown || (self.printed && self.shared_screen()) {
            let _ = writeln!(self.err);
        }
        self.err_shown = true;
        self.answering = false;
        self.thinking = false;
        self.held_newlines = 0;
    }

    /// Starts a part of the answer after other messages. On a shared
    /// terminal the blank line goes to `err`, so `out` holds only the answer.
    pub(super) fn begin_answer(&mut self) {
        self.thinking = false;
        self.held_newlines = 0;
        if std::mem::replace(&mut self.answering, true) {
            return;
        }
        if self.shared_screen() {
            self.close_open_line();
            if self.err_shown || self.printed {
                let _ = writeln!(self.err);
            }
        } else if self.printed {
            let gap = if self.answer_ends_line { "\n" } else { "\n\n" };
            let _ = write!(self.out, "{gap}");
        }
    }
}
