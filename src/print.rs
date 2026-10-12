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
    while let Some(message) = agent.next_background().await {
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

    /// Ends a line left open on `out` or `err` with a newline on `err`.
    fn close_open_line(&mut self) {
        if std::mem::take(&mut self.err_open) | std::mem::take(&mut self.out_open) {
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
mod tests {
    use super::{AgentEvent, Printer};
    use crate::agent::test_support::{self, MockApi, Reply, text_reply, tool_reply};
    use crate::tools::test_support::TemporaryDir;
    use serde_json::json;
    use std::io;

    const ESCAPES: &str = "\x1b]52;c;aGk=\x07\x1b[2J\u{9b}2J\r";

    /// Removes the colour codes print mode writes itself.
    fn without_colours(text: &str) -> String {
        let mut plain = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("\x1b[") {
            plain.push_str(&rest[..start]);
            let code = &rest[start + 2..];
            match code.find(|c: char| !c.is_ascii_digit() && c != ';') {
                Some(end) if code[end..].starts_with(['m', 'K']) => rest = &code[end + 1..],
                _ => {
                    plain.push_str("\x1b[");
                    rest = code;
                }
            }
        }
        plain.push_str(rest);
        plain
    }

    fn has_controls(text: &str) -> bool {
        text.chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    }

    #[test]
    fn separates_thinking_tool_calls_and_notices_with_blank_lines() {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
        printer.event(AgentEvent::Thinking("let me".into()));
        printer.event(AgentEvent::Thinking("\tthink".into()));
        printer.event(AgentEvent::ToolStart {
            name: "bash".into(),
            summary: String::new(),
            diff: Some("ls".into()),
        });
        printer.event(AgentEvent::ToolDone {
            name: "bash".into(),
            error: Some("boom".into()),
            note: None,
        });
        printer.event(AgentEvent::Notice("note".into()));
        printer.event(AgentEvent::Text("answer".into()));
        printer.finish(Ok(())).unwrap();
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "let me\tthink\n\nbash\nls\n\nbash failed: boom\n\nnote\n"
        );
        assert_eq!(String::from_utf8(out).unwrap(), "answer\n");
    }

    #[test]
    fn leaves_one_blank_line_after_thinking_that_ends_with_newlines() {
        let mut err = Vec::new();
        let mut printer = Printer::new(io::sink(), false, &mut err, false, false, None);
        printer.event(AgentEvent::Thinking("one\n\n".into()));
        printer.event(AgentEvent::Thinking("two.\n".into()));
        printer.event(AgentEvent::Thinking("\n".into()));
        printer.event(AgentEvent::Notice("note".into()));
        printer.event(AgentEvent::Thinking("last\n\n".into()));
        printer.finish(Err(anyhow::anyhow!("boom"))).unwrap_err();
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "one\n\ntwo.\n\nnote\n\nlast\n"
        );
    }

    #[test]
    fn shows_thinking_in_grey_italics() {
        let mut err = Vec::new();
        let mut printer = Printer::new(io::sink(), true, &mut err, true, true, None);
        printer.event(AgentEvent::Thinking("hmm\x1b[2J".into()));
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "\x1b[90m\x1b[3mhmm[2J\x1b[0m"
        );
    }

    #[test]
    fn leaves_one_blank_line_before_the_answer_on_a_shared_terminal() {
        struct Screen<'a>(&'a std::cell::RefCell<Vec<u8>>);
        impl io::Write for Screen<'_> {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let screen = std::cell::RefCell::new(Vec::new());
        let mut printer = Printer::new(Screen(&screen), true, Screen(&screen), true, true, None);
        printer.event(AgentEvent::Text("first".into()));
        printer.event(AgentEvent::Notice("note".into()));
        printer.event(AgentEvent::Text("second".into()));
        printer.finish(Ok(())).unwrap();
        drop(printer);
        let shown = without_colours(&String::from_utf8(screen.into_inner()).unwrap());
        assert_eq!(shown, "first\n\nnote\n\nsecond\n");
    }

    #[tokio::test]
    async fn keeps_escape_sequences_from_the_model_and_tools_out_of_the_terminal() {
        let directory = TemporaryDir::new("print-escape");
        let path = directory.path().join("escape");
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "write",
                json!({ "path": path, "content": format!("one{ESCAPES}\ntwo\n") }),
            ),
            tool_reply(
                "call-2",
                "bash",
                json!({
                    "command": format!("printf 'out\\033]52;c;aGk=\\007\\033[2J'; sleep 5 # {ESCAPES}"),
                    "timeout": 1,
                }),
            ),
            text_reply(&format!("done{ESCAPES}")),
        ])
        .await;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut printer = Printer::new(&mut out, true, &mut err, true, true, None);
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        printer.flush_reads();
        printer.finish(result).unwrap();
        let out = String::from_utf8(out).unwrap();
        let err = String::from_utf8(err).unwrap();

        assert_eq!(out, "done]52;c;aGk=[2J2J\n");
        assert!(err.contains("\x1b[38;2;"), "colours missing: {err:?}");
        let plain = without_colours(&err);
        assert!(!has_controls(&plain), "control characters in {plain:?}");
        assert!(plain.contains(" one]52;c;aGk=[2J2J"), "{plain:?}");
        assert!(plain.contains(" bash  printf"), "{plain:?}");
        assert!(plain.contains("bash failed: out]52;c;aGk=[2J"), "{plain:?}");
    }

    #[tokio::test]
    async fn answers_each_background_command_when_it_finishes() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "bash",
                json!({ "command": "sleep 0.3; echo finished", "run_in_background": true }),
            ),
            text_reply("waiting"),
            text_reply("seen it"),
        ])
        .await;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut checkpoint = None;
        let result = super::run(&mut agent, &mut printer, "go", &[], &mut checkpoint).await;
        test_support::remove_session(&agent);
        printer.finish(result).unwrap();
        assert_eq!(checkpoint, None);
        assert_eq!(String::from_utf8(out).unwrap(), "waiting\n\nseen it\n");
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.contains("background task 1 finished\nfinished"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn uses_github_high_contrast_colours() {
        for (dark, keyword) in [(true, "255;148;146"), (false, "160;17;31")] {
            let api = MockApi::start(vec![
                tool_reply(
                    "call-1",
                    "bash",
                    json!({ "command": "python3 - <<'EOF'\ndef f():\n    pass\nEOF" }),
                ),
                text_reply("done"),
            ])
            .await;
            let mut err = Vec::new();
            let mut printer = Printer::new(io::sink(), true, &mut err, true, dark, None);
            let (_, result) =
                test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
            let finished = printer.finish(result);
            finished.unwrap();
            let err = String::from_utf8(err).unwrap();
            assert!(
                err.contains(&format!("\x1b[38;2;{keyword}mdef")),
                "dark {dark}: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn highlights_bash_commands() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "bash",
                json!({ "command": "echo \"$HOME\" | wc -c" }),
            ),
            text_reply("done"),
        ])
        .await;
        let mut err = Vec::new();
        let mut printer = Printer::new(io::sink(), true, &mut err, true, true, None);
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        let finished = printer.finish(result);
        finished.unwrap();
        let err = String::from_utf8(err).unwrap();
        let plain = without_colours(&err);
        assert!(
            plain.contains(" bash  echo \"$HOME\" | wc -c\n"),
            "{plain:?}"
        );
        let command = err.lines().find(|line| line.contains("echo")).unwrap();
        assert!(command.matches("\x1b[38;2;").count() > 1, "{command:?}");
    }

    #[tokio::test]
    async fn fills_tool_calls_with_a_background_near_the_terminal_one() {
        let api = MockApi::start(vec![
            tool_reply("call-1", "bash", json!({ "command": "echo hi" })),
            text_reply("done"),
        ])
        .await;
        let mut err = Vec::new();
        let mut printer = Printer::new(io::sink(), true, &mut err, true, true, Some((30, 30, 30)));
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        printer.finish(result).unwrap();
        let err = String::from_utf8(err).unwrap();
        let lines: Vec<&str> = err.lines().take(1).collect();
        assert!(
            lines[0].starts_with("\x1b[0m\x1b[48;2;50;50;50m\x1b[38;2;220;220;220m\x1b[1m bash "),
            "{lines:?}"
        );
        for line in &lines {
            assert!(
                line.ends_with("\x1b[48;2;42;42;42m\x1b[K\x1b[0m"),
                "{lines:?}"
            );
        }
        assert_eq!(
            without_colours(&err).lines().take(1).collect::<Vec<_>>(),
            [" bash  echo hi"]
        );
    }

    #[tokio::test]
    async fn writes_piped_answers_byte_for_byte() {
        let answer = format!("done{ESCAPES}\r\n");
        let api = MockApi::start(vec![text_reply(&answer)]).await;
        let mut out = Vec::new();
        let mut printer = Printer::new(&mut out, false, io::sink(), false, false, None);
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        let finished = printer.finish(result);
        finished.unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("{answer}\n"));
    }

    #[tokio::test]
    async fn ends_an_answer_that_broke_off_before_the_error_is_shown() {
        let api = MockApi::start(vec![
            Reply::Events(vec![
                json!({ "type": "message_start", "message": { "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "partial" } }),
                json!({ "type": "error", "error": { "type": "overloaded_error", "message": "Overloaded" } }),
            ]),
            Reply::BadRequest("bad".into()),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut outputs = Vec::new();
        for _ in 0..2 {
            let mut out = Vec::new();
            let mut printer = Printer::new(&mut out, false, io::sink(), false, false, None);
            let result = agent.prompt("go", &[], |event| printer.event(event)).await;
            let finished = printer.finish(result);
            outputs.push((String::from_utf8(out).unwrap(), finished.is_err()));
        }
        test_support::remove_session(&agent);

        // The broken-off answer ends its line, so the error starts on its
        // own; a run that printed nothing adds no blank line.
        assert_eq!(
            outputs,
            [("partial\n".to_string(), true), (String::new(), true)]
        );
    }

    #[tokio::test]
    async fn prints_only_the_tasks_and_why_one_failed() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "sequential",
                    "tasks": [
                        { "description": "say hi", "prompt": "say hi" },
                        { "description": "say bye", "prompt": "say bye" },
                    ],
                }),
            ),
            tool_reply("call-2", "bash", json!({ "command": "true" })),
            text_reply("hi"),
            Reply::BadRequest("bad".into()),
            text_reply("done"),
        ])
        .await;
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        let finished = printer.finish(result);
        finished.unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "done\n");
        let err = String::from_utf8(err).unwrap();
        assert!(
            err.starts_with("agent sequential, 2 tasks\n\n[agent 1] say hi · 1 tool call · "),
            "{err}"
        );
        assert!(
            err.contains("\n\n[agent 2] say bye\n  This task failed: 400 Bad Request: "),
            "{err}"
        );
    }
}
