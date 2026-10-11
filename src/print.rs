use crate::{agent::AgentEvent, highlight, tools};
use std::io::Write;

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
    separate: bool,
    line_open: bool,
    reads: tools::ReadGroup,
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
            separate: false,
            line_open: false,
            reads: tools::ReadGroup::default(),
        }
    }

    pub fn event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Text(text) => {
                self.flush_reads();
                if self.separate {
                    let _ = write!(self.out, "\n\n");
                    self.separate = false;
                }
                if self.out_terminal {
                    let _ = write!(self.out, "{}", highlight::strip_controls(&text));
                } else {
                    let _ = write!(self.out, "{text}");
                }
                let _ = self.out.flush();
                self.printed = true;
                self.line_open = !text.ends_with('\n');
            }
            AgentEvent::ToolStart {
                name,
                summary,
                diff,
            } => {
                self.separate = self.printed;
                if std::mem::take(&mut self.line_open) {
                    let _ = writeln!(self.err);
                }
                if name == "read" && diff.is_none() {
                    self.reads.add(summary);
                    return;
                }
                self.flush_reads();
                self.tool(&name, summary, diff);
            }
            AgentEvent::ToolDone {
                name,
                error: Some(error),
                ..
            } => {
                self.flush_reads();
                if std::mem::take(&mut self.line_open) {
                    let _ = writeln!(self.err);
                }
                self.line(&format!("{name} failed: {error}"));
            }
            AgentEvent::ToolDone {
                name,
                note: Some(note),
                ..
            } => {
                self.flush_reads();
                self.line(&format!("{name}: {note}"));
            }
            AgentEvent::Notice(text) => {
                self.flush_reads();
                self.line(&format!("\n{text}"));
            }
            AgentEvent::TaskStart { task, description } => {
                self.task_line(task, &description);
            }
            AgentEvent::TaskDone {
                task,
                reply,
                tool_calls,
            } => {
                self.task_line(task, &tools::agent::tool_calls(tool_calls));
                if let Err(error) = reply {
                    let line = error.lines().next().unwrap_or_default();
                    self.task_line(task, line);
                }
            }
            _ => {}
        }
    }

    fn task_line(&mut self, task: usize, text: &str) {
        self.flush_reads();
        for line in text.lines() {
            self.line(&format!("  [agent {}] {line}", task + 1));
        }
    }

    pub fn flush_reads(&mut self) {
        if !self.reads.is_empty() {
            let summary = self.reads.summary();
            self.reads.clear();
            self.tool("read", summary, None);
        }
    }

    /// Ends the answer with a newline once the run has finished. A failed
    /// run ends the answer it started too, so the error shown next starts
    /// on its own line.
    pub fn finish(&mut self, result: anyhow::Result<()>) -> anyhow::Result<()> {
        let ended = if result.is_ok() || self.printed {
            writeln!(self.out)
        } else {
            Ok(())
        };
        result?;
        Ok(ended?)
    }

    fn tool(&mut self, name: &str, summary: String, diff: Option<String>) {
        if !self.colour {
            if summary.is_empty() {
                self.line(name);
            } else {
                self.line(&format!("{name} {summary}"));
            }
            if let Some(diff) = diff {
                self.line(&diff);
            }
            return;
        }
        let columns = crossterm::terminal::size().ok().map(|(columns, _)| columns);
        let text = crate::tui::tool_message(name, summary, diff);
        for line in crate::tui::ansi_tool_lines(&text, self.dark, self.background, columns) {
            let _ = writeln!(self.err, "{line}");
        }
    }

    fn line(&mut self, text: &str) {
        let _ = writeln!(self.err, "{}", highlight::strip_controls(text));
    }
}

#[cfg(test)]
mod tests {
    use super::Printer;
    use crate::agent::test_support::{self, MockApi, Reply, text_reply, tool_reply};
    use serde_json::json;

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

    #[tokio::test]
    async fn keeps_escape_sequences_from_the_model_and_tools_out_of_the_terminal() {
        let path = std::env::temp_dir().join(format!(
            "rust-coding-agent-print-escape-{}",
            std::process::id()
        ));
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
        let finished = printer.finish(result);
        let _ = std::fs::remove_file(&path);
        finished.unwrap();
        let out = String::from_utf8(out).unwrap();
        let err = String::from_utf8(err).unwrap();

        assert_eq!(out, "done]52;c;aGk=[2J2J\n");
        assert!(err.contains("\x1b[38;2;"), "colours missing: {err:?}");
        let plain = without_colours(&err);
        assert!(!has_controls(&plain), "control characters in {plain:?}");
        assert!(plain.contains(" one]52;c;aGk=[2J2J"), "{plain:?}");
        assert!(plain.contains(" bash \nprintf"), "{plain:?}");
        assert!(plain.contains("bash failed: out]52;c;aGk=[2J"), "{plain:?}");
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
            let mut printer = Printer::new(std::io::sink(), true, &mut err, true, dark, None);
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
        let mut printer = Printer::new(std::io::sink(), true, &mut err, true, true, None);
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        let finished = printer.finish(result);
        finished.unwrap();
        let err = String::from_utf8(err).unwrap();
        let plain = without_colours(&err);
        assert!(
            plain.contains(" bash \necho \"$HOME\" | wc -c\n"),
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
        let mut printer = Printer::new(
            std::io::sink(),
            true,
            &mut err,
            true,
            true,
            Some((30, 30, 30)),
        );
        let (_, result) = test_support::run_prompt(&api, "go", |event| printer.event(event)).await;
        printer.finish(result).unwrap();
        let err = String::from_utf8(err).unwrap();
        let lines: Vec<&str> = err.lines().take(2).collect();
        assert!(
            lines[0].starts_with("\x1b[0m\x1b[48;2;50;50;50m\x1b[38;2;220;220;220m bash "),
            "{lines:?}"
        );
        for line in &lines {
            assert!(
                line.ends_with("\x1b[48;2;42;42;42m\x1b[K\x1b[0m"),
                "{lines:?}"
            );
        }
        assert_eq!(
            without_colours(&err).lines().take(2).collect::<Vec<_>>(),
            [" bash ", "echo hi"]
        );
    }

    #[tokio::test]
    async fn writes_piped_answers_byte_for_byte() {
        let answer = format!("done{ESCAPES}\r\n");
        let api = MockApi::start(vec![text_reply(&answer)]).await;
        let mut out = Vec::new();
        let mut printer = Printer::new(&mut out, false, std::io::sink(), false, false, None);
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
            let mut printer = Printer::new(&mut out, false, std::io::sink(), false, false, None);
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
        let expected = "agent sequential, 2 tasks\n  [agent 1] say hi\n  [agent 1] 1 tool call\n  [agent 2] say bye\n  [agent 2] 0 tool calls\n  [agent 2] This task failed: 400 Bad Request: ";
        assert!(err.starts_with(expected), "{err}");
    }
}
