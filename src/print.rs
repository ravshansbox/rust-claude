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
    printed: bool,
    separate: bool,
    line_open: bool,
    reads: tools::ReadGroup,
}

impl<Out: Write, Err: Write> Printer<Out, Err> {
    pub fn new(out: Out, out_terminal: bool, err: Err, colour: bool, dark: bool) -> Self {
        Self {
            out,
            err,
            out_terminal,
            colour,
            dark,
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
                if self.line_open {
                    let _ = writeln!(self.err);
                    self.line_open = false;
                }
                if name == "read" && diff.is_none() {
                    self.reads.add(summary);
                    return;
                }
                self.flush_reads();
                if summary.is_empty() {
                    self.line(&name);
                } else {
                    self.line(&format!("{name} {summary}"));
                }
                if let Some(diff) = diff {
                    match highlight::highlight_tool(&name, &summary, &diff, self.dark) {
                        Some(lines) if self.colour => {
                            for line in lines {
                                let _ = writeln!(
                                    self.err,
                                    "{}",
                                    highlight::ansi_line(&line, self.dark)
                                );
                            }
                        }
                        _ => self.line(&diff),
                    }
                }
            }
            AgentEvent::ToolDone {
                name,
                error: Some(error),
                ..
            } => {
                self.flush_reads();
                if self.line_open {
                    let _ = writeln!(self.err);
                    self.line_open = false;
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
            _ => {}
        }
    }

    pub fn flush_reads(&mut self) {
        if !self.reads.is_empty() {
            let summary = format!("read {}", self.reads.summary());
            self.line(&summary);
            self.reads.clear();
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
                Some(end) if code[end..].starts_with('m') => rest = &code[end + 1..],
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
        let path =
            std::env::temp_dir().join(format!("rust-claude-print-escape-{}", std::process::id()));
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
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut printer = Printer::new(&mut out, true, &mut err, true, true);
        let result = agent.prompt("go", &[], |event| printer.event(event)).await;
        printer.flush_reads();
        let finished = printer.finish(result);
        test_support::remove_session(&agent);
        let _ = std::fs::remove_file(&path);
        finished.unwrap();
        let out = String::from_utf8(out).unwrap();
        let err = String::from_utf8(err).unwrap();

        assert_eq!(out, "done]52;c;aGk=[2J2J\n");
        assert!(err.contains("\x1b[39m"), "colours missing: {err:?}");
        let plain = without_colours(&err);
        assert!(!has_controls(&plain), "control characters in {plain:?}");
        assert!(plain.contains(" one]52;c;aGk=[2J2J"), "{plain:?}");
        assert!(plain.contains("bash printf"), "{plain:?}");
        assert!(plain.contains("bash failed: out]52;c;aGk=[2J"), "{plain:?}");
    }

    #[tokio::test]
    async fn highlights_python_code() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "python",
                json!({ "code": "def f():\n    return 1\nprint(f())" }),
            ),
            text_reply("done"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut err = Vec::new();
        let mut printer = Printer::new(std::io::sink(), true, &mut err, true, true);
        let result = agent.prompt("go", &[], |event| printer.event(event)).await;
        let finished = printer.finish(result);
        test_support::remove_session(&agent);
        finished.unwrap();
        let err = String::from_utf8(err).unwrap();
        let plain = without_colours(&err);
        assert!(
            plain.contains("python\n def f():\n     return 1\n print(f())\n"),
            "{plain:?}"
        );
        let definition = err.lines().find(|line| line.contains("def")).unwrap();
        assert!(
            definition.matches("\x1b[38;2;").count() > 1,
            "{definition:?}"
        );
    }

    #[tokio::test]
    async fn writes_piped_answers_byte_for_byte() {
        let answer = format!("done{ESCAPES}\r\n");
        let api = MockApi::start(vec![text_reply(&answer)]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut out = Vec::new();
        let mut printer = Printer::new(&mut out, false, std::io::sink(), false, false);
        let result = agent.prompt("go", &[], |event| printer.event(event)).await;
        let finished = printer.finish(result);
        test_support::remove_session(&agent);
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
            let mut printer = Printer::new(&mut out, false, std::io::sink(), false, false);
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
}
