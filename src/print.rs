use crate::{
    agent::{Agent, AgentEvent, Stats},
    highlight,
    images::Image,
    tools, tui,
};
use std::{collections::HashMap, io::Write};

/// Answers `prompt`, then each background command as it finishes. While a
/// turn runs, `checkpoint` holds where it started, so a stop can cancel it.
pub async fn run<Out: Write, Err: Write>(
    agent: &mut Agent,
    printer: &mut Printer<Out, Err>,
    prompt: &str,
    images: &[Image],
    checkpoint: &mut Option<usize>,
) -> anyhow::Result<()> {
    turn(agent, printer, prompt, images, checkpoint).await?;
    while let Some(message) = agent.background.next().await {
        printer.flush_reads();
        if let Some((summary, output)) = tools::background::display(&message) {
            printer.tool(&tui::tool_message(
                "background",
                &summary,
                output.as_deref(),
            ));
        }
        turn(agent, printer, &message, &[], checkpoint).await?;
    }
    Ok(())
}

async fn turn<Out: Write, Err: Write>(
    agent: &mut Agent,
    printer: &mut Printer<Out, Err>,
    prompt: &str,
    images: &[Image],
    checkpoint: &mut Option<usize>,
) -> anyhow::Result<()> {
    *checkpoint = Some(agent.history_len());
    agent
        .prompt(prompt, images, |event| printer.event(event))
        .await?;
    *checkpoint = None;
    Ok(())
}

/// Writes a print mode run: the answer goes to `out`, tool activity and
/// notices to `err`.
pub struct Printer<Out, Err> {
    out: Out,
    err: Err,
    /// Whether `out` is a terminal. Piped answers are written byte for byte;
    /// only a terminal gets control characters removed, since there they
    /// could clear the screen or write to the clipboard.
    out_terminal: bool,
    colour: bool,
    dark: bool,
    background: Option<highlight::Colour>,
    printed: bool,
    /// Whether the text written last to `out` or `err` did not end its line.
    out_open: bool,
    err_open: bool,
    err_shown: bool,
    answering: bool,
    thinking: bool,
    held_newlines: usize,
    reads: tools::ReadGroup,
    tasks: HashMap<usize, (String, Option<Stats>)>,
}

impl<Out: Write, Err: Write> Printer<Out, Err> {
    pub fn new(
        out: Out,
        out_terminal: bool,
        err: Err,
        colour: bool,
        dark: bool,
        background: Option<highlight::Colour>,
    ) -> Self {
        Self {
            out,
            err,
            out_terminal,
            colour,
            dark,
            background,
            printed: false,
            out_open: false,
            err_open: false,
            err_shown: false,
            answering: false,
            thinking: false,
            held_newlines: 0,
            reads: tools::ReadGroup::default(),
            tasks: HashMap::new(),
        }
    }

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
    fn begin_err(&mut self) {
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
    fn begin_answer(&mut self) {
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
            let _ = write!(self.out, "\n\n");
        }
    }

    pub fn event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Text(text) => {
                self.flush_reads();
                self.begin_answer();
                if self.out_terminal {
                    let _ = write!(self.out, "{}", highlight::strip_controls(&text));
                } else {
                    let _ = write!(self.out, "{text}");
                }
                let _ = self.out.flush();
                self.printed = true;
                self.out_open = !text.ends_with('\n');
            }
            AgentEvent::Thinking(text) => {
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
            AgentEvent::ToolStart {
                name,
                summary,
                diff,
            } => {
                if name == "read" && diff.is_none() {
                    self.answering = false;
                    self.thinking = false;
                    self.reads.add(summary);
                    return;
                }
                self.flush_reads();
                self.tool(&tui::tool_message(&name, &summary, diff.as_deref()));
            }
            AgentEvent::ToolDone {
                name,
                error: Some(error),
                ..
            } => {
                self.flush_reads();
                self.message(&format!("{name} failed: {error}"));
            }
            AgentEvent::ToolDone {
                name,
                note: Some(note),
                ..
            } => {
                self.flush_reads();
                self.message(&format!("{name}: {note}"));
            }
            AgentEvent::Notice(text) => {
                self.flush_reads();
                self.message(&text);
            }
            AgentEvent::TaskStart { task, description } => {
                self.tasks.insert(task, (description, None));
            }
            AgentEvent::TaskStats { task, stats } => {
                if let Some((_, task_stats)) = self.tasks.get_mut(&task) {
                    *task_stats = Some(stats);
                }
            }
            AgentEvent::TaskDone {
                task,
                reply,
                tool_calls,
            } => {
                self.flush_reads();
                let (description, stats) = self.tasks.remove(&task).unwrap_or_default();
                let failure = reply.err();
                let text = tui::finished_task(
                    task,
                    &description,
                    tool_calls,
                    stats.as_ref(),
                    failure.as_deref(),
                );
                self.tool(&text);
            }
            _ => {}
        }
    }

    pub fn flush_reads(&mut self) {
        if !self.reads.is_empty() {
            let summary = std::mem::take(&mut self.reads).summary();
            self.tool(&tui::tool_message("read", &summary, None));
        }
    }

    /// Ends the answer with a newline once the run has finished. A failed
    /// run ends the answer it started too, so the error shown next starts
    /// on its own line.
    pub fn finish(&mut self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        if std::mem::take(&mut self.err_open) {
            let _ = writeln!(self.err);
        }
        let ended = if result.is_ok() || self.printed {
            writeln!(self.out)
        } else {
            Ok(())
        };
        result?;
        Ok(ended?)
    }

    /// Writes a tool call or task as the interface shows it, or as plain
    /// text when `err` is not a terminal.
    fn tool(&mut self, text: &str) {
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

    fn message(&mut self, text: &str) {
        self.begin_err();
        self.line(text);
    }

    fn line(&mut self, text: &str) {
        let _ = writeln!(self.err, "{}", highlight::strip_controls(text));
    }
}

#[cfg(test)]
mod tests;
