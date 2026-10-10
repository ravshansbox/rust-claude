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
                self.line(&format!("{name} {summary}"));
                if let Some(diff) = diff {
                    if self.colour {
                        for line in highlight::highlight_body(&summary, &diff, self.dark) {
                            let _ =
                                writeln!(self.err, "{}", highlight::ansi_line(&line, self.dark));
                        }
                    } else {
                        self.line(&diff);
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

    /// Ends the answer with a newline once the run has finished.
    pub fn finish(&mut self) -> std::io::Result<()> {
        writeln!(self.out)
    }

    fn line(&mut self, text: &str) {
        let _ = writeln!(self.err, "{}", highlight::strip_controls(text));
    }
}

#[cfg(test)]
mod tests {
    use super::Printer;
    use crate::agent::test_support::{self, MockApi, text_reply, tool_reply};
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
        printer.finish().unwrap();
        test_support::remove_session(&agent);
        let _ = std::fs::remove_file(&path);
        result.unwrap();
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
    async fn writes_piped_answers_byte_for_byte() {
        let answer = format!("done{ESCAPES}\r\n");
        let api = MockApi::start(vec![text_reply(&answer)]).await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut out = Vec::new();
        let mut printer = Printer::new(&mut out, false, std::io::sink(), false, false);
        let result = agent.prompt("go", &[], |event| printer.event(event)).await;
        printer.finish().unwrap();
        test_support::remove_session(&agent);
        result.unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), format!("{answer}\n"));
    }
}
