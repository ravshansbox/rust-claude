use crate::{
    agent::{AgentEvent, Stats},
    highlight, tools, tui,
};
use std::{collections::HashMap, io::Write};

mod messages;
mod spacing;
mod streams;

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

    pub fn event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Text(text) => self.answer_text(&text),
            AgentEvent::Thinking(text) => self.thinking_text(&text),
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
            AgentEvent::ToolDone { name, error, note } => {
                let text = match (error, note) {
                    (Some(error), _) => tui::tool_failure(&name, &error),
                    (None, Some(note)) => tui::tool_note(&name, &note),
                    (None, None) => return,
                };
                self.flush_reads();
                self.message(&text);
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
}
