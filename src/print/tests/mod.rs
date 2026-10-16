use super::Printer;
use crate::agent::AgentEvent;
use std::{cell::RefCell, io};

mod colours;
mod escapes;
mod runs;

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
fn keeps_the_answer_line_out_of_a_separate_error_stream() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, true, &mut err, false, false, None);
    for (text, command) in [("I'll look.", "ls"), ("Now another.", "pwd")] {
        printer.event(AgentEvent::Text(text.into()));
        printer.event(AgentEvent::ToolStart {
            name: "bash".into(),
            summary: String::new(),
            diff: Some(command.into()),
        });
    }
    printer.event(AgentEvent::Text("Done.".into()));
    printer.finish(Ok(())).unwrap();
    assert_eq!(String::from_utf8(err).unwrap(), "bash\nls\n\nbash\npwd\n");
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "I'll look.\n\nNow another.\n\nDone.\n"
    );
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
fn ends_the_thinking_line_before_the_answer_when_both_streams_are_piped_together() {
    struct Log<'a>(&'a RefCell<Vec<u8>>);
    impl io::Write for Log<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let log = RefCell::new(Vec::new());
    let mut printer = Printer::new(Log(&log), false, Log(&log), false, false, None);
    printer.event(AgentEvent::Thinking("hmm".into()));
    printer.event(AgentEvent::Text("answer".into()));
    printer.finish(Ok(())).unwrap();
    drop(printer);
    assert_eq!(
        String::from_utf8(log.into_inner()).unwrap(),
        "hmm\nanswer\n"
    );
}

#[test]
fn leaves_one_blank_line_before_the_answer_on_a_shared_terminal() {
    struct Screen<'a>(&'a RefCell<Vec<u8>>);
    impl io::Write for Screen<'_> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let screen = RefCell::new(Vec::new());
    let mut printer = Printer::new(Screen(&screen), true, Screen(&screen), true, true, None);
    printer.event(AgentEvent::Text("first".into()));
    printer.event(AgentEvent::Notice("note".into()));
    printer.event(AgentEvent::Text("second".into()));
    printer.finish(Ok(())).unwrap();
    drop(printer);
    let shown = without_colours(&String::from_utf8(screen.into_inner()).unwrap());
    assert_eq!(shown, "first\n\nnote\n\nsecond\n");
}

#[test]
fn groups_reads_in_a_row_and_shows_tool_notes() {
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut printer = Printer::new(&mut out, false, &mut err, false, false, None);
    for path in ["src/main.rs", "src/lib.rs"] {
        printer.event(AgentEvent::ToolStart {
            name: "read".into(),
            summary: path.into(),
            diff: None,
        });
    }
    printer.event(AgentEvent::ToolDone {
        name: "edit".into(),
        error: None,
        note: Some("2 replacements".into()),
    });
    printer.event(AgentEvent::ToolStart {
        name: "read".into(),
        summary: "README.md".into(),
        diff: None,
    });
    printer.flush_reads();
    printer.finish(Ok(())).unwrap();
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "read src/main.rs, src/lib.rs\n\nedit: 2 replacements\n\nread README.md\n"
    );
}

#[test]
fn leaves_one_blank_line_after_an_answer_part_that_ends_its_line() {
    let mut out = Vec::new();
    let mut printer = Printer::new(&mut out, false, io::sink(), false, false, None);
    printer.event(AgentEvent::Text("I'll look.\n".into()));
    printer.event(AgentEvent::ToolStart {
        name: "bash".into(),
        summary: String::new(),
        diff: Some("ls".into()),
    });
    printer.event(AgentEvent::Text("Done.".into()));
    printer.finish(Ok(())).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "I'll look.\n\nDone.\n");
}

#[test]
fn does_not_add_a_blank_line_after_an_answer_that_ends_its_line() {
    let mut out = Vec::new();
    let mut printer = Printer::new(&mut out, false, io::sink(), false, false, None);
    printer.event(AgentEvent::Text("Done.\n".into()));
    printer.finish(Ok(())).unwrap();
    assert_eq!(String::from_utf8(out).unwrap(), "Done.\n");
}
