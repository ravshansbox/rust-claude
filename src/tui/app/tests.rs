use super::App;
use crate::agent::AgentEvent;
use crate::tools::test_support::TemporaryDir;
use crate::tui::message::Role;
use crate::tui::{
    UiEvent,
    keys::handle_input,
    replay::handle_agent_event,
    test_support::{new_app, press, screen},
};
use crossterm::event::{Event, KeyCode};
use ratatui::text::Line;

fn stream_in_chunks(reply: &str, size: usize) -> App {
    let mut app = new_app();
    let characters: Vec<char> = reply.chars().collect();
    for chunk in characters.chunks(size) {
        let text: String = chunk.iter().collect();
        handle_agent_event(UiEvent::Agent(AgentEvent::Text(text)), &mut app);
        screen(&mut app);
    }
    app
}

#[test]
fn merges_consecutive_reads() {
    let mut app = new_app();
    app.push_tool("read", "a.rs".into(), None);
    app.push_tool("read", "b.rs".into(), None);
    app.push_tool("read", "a.rs".into(), None);
    app.push(Role::Event, "read failed: missing");
    app.push_tool("read", "c.rs".into(), None);
    let texts: Vec<&str> = app.messages[1..]
        .iter()
        .map(|message| message.text.as_str())
        .collect();
    assert_eq!(
        texts,
        ["read a.rs (2), b.rs", "read failed: missing", "read c.rs"]
    );
}

#[test]
fn renders_a_streamed_reply_the_same_as_the_whole_reply() {
    let reply = "Intro with **bold** text.\n\n```rust\nfn main() {}\n```\n\n\
        1. Step one\n   ```sh\n   cargo test\n   ```\n2. Step two\n\n\
        # Heading\n```\nno blank line before or after\n```\nafter\n\
        | a | b |\n| --- | --- |\n| 1 | 2 |\n\n~~~\ntilde\n~~~\n\n\
        > quote\n\n```python\nprint('x')\n```\n\n```\n~~~\n```\nfirst\nsecond\n";
    let mut streamed = stream_in_chunks(reply, 3);
    let mut whole = new_app();
    whole.push(Role::Assistant, reply);
    let shown = screen(&mut whole);
    let rendered = |app: &App| {
        let rendered = app.messages.last().unwrap().rendered.as_ref().unwrap();
        (rendered.lines.clone(), rendered.heights.clone())
    };
    assert_eq!(rendered(&streamed), rendered(&whole));
    assert_eq!(screen(&mut streamed), shown);
}

#[test]
fn renders_streamed_blocks_split_by_blank_lines_the_same_as_the_whole_reply() {
    let replies = [
        "- one\n\n- two\n\n  still two\n\nafter the list\n",
        "<!--\n\nhidden\n\n-->\n\nafter html\n",
        "Intro\n\nTitle\n---\n\nOther\n===\n\nend\n",
        "Code:\n\n    let a = 1;\n\n    let b = 2;\n\nafter code\n",
        "> quoted\n\n> more quote\n\nlazy\ncontinued\n",
        "Title\n---\n    ```\n   ```\n~~~\n~~~\n```\ntext\n   ```\nmore code\n",
    ];
    for reply in replies {
        let mut streamed = stream_in_chunks(reply, 1);
        let mut whole = new_app();
        whole.push(Role::Assistant, reply);
        assert_eq!(screen(&mut streamed), screen(&mut whole), "{reply}");
    }
}

#[test]
fn keeps_finished_paragraphs_of_a_streaming_reply() {
    let mut app = new_app();
    let stream = |app: &mut App, text: &str| {
        handle_agent_event(UiEvent::Agent(AgentEvent::Text(text.into())), app);
        screen(app);
    };
    stream(&mut app, "First paragraph.\n\nSecond\n");
    let rendered = app.messages.last_mut().unwrap().rendered.as_mut().unwrap();
    rendered.lines[0] = Line::raw("cached");
    stream(&mut app, " paragraph goes on");
    let rendered = app.messages.last().unwrap().rendered.as_ref().unwrap();
    assert_eq!(rendered.lines[0], Line::raw("cached"));
}

#[test]
fn keeps_finished_paragraphs_after_inline_code_that_starts_a_line() {
    let mut app = new_app();
    let stream = |app: &mut App, text: &str| {
        handle_agent_event(UiEvent::Agent(AgentEvent::Text(text.into())), app);
        screen(app);
    };
    stream(&mut app, "```a` is inline code.\n\nSecond\n");
    let rendered = app.messages.last_mut().unwrap().rendered.as_mut().unwrap();
    rendered.lines[0] = Line::raw("cached");
    stream(&mut app, " paragraph goes on");
    let rendered = app.messages.last().unwrap().rendered.as_ref().unwrap();
    assert_eq!(rendered.lines[0], Line::raw("cached"));
}

#[test]
fn keeps_finished_paragraphs_after_a_quoted_code_block_left_open() {
    let mut app = new_app();
    let stream = |app: &mut App, text: &str| {
        handle_agent_event(UiEvent::Agent(AgentEvent::Text(text.into())), app);
        screen(app);
    };
    stream(&mut app, "> ```\n> code\nafter the quote\n\nSecond\n");
    let rendered = app.messages.last_mut().unwrap().rendered.as_mut().unwrap();
    rendered.lines[0] = Line::raw("cached");
    stream(&mut app, " paragraph goes on");
    let rendered = app.messages.last().unwrap().rendered.as_ref().unwrap();
    assert_eq!(rendered.lines[0], Line::raw("cached"));
}

#[test]
fn renders_a_finished_reply_the_same_as_the_whole_reply() {
    let reply =
        "See [docs] and[^1].\n\nMore text.\n\n[docs]: https://example.com\n\n[^1]: A note.\n";
    let mut streamed = stream_in_chunks(reply, 3);
    handle_agent_event(UiEvent::Done(Ok(())), &mut streamed);
    let mut whole = new_app();
    whole.push(Role::Assistant, reply);
    handle_agent_event(UiEvent::Done(Ok(())), &mut whole);
    assert_eq!(screen(&mut streamed), screen(&mut whole));
}

#[test]
fn sends_a_prompt_while_another_copy_holds_the_history_lock() {
    let folder = TemporaryDir::new("history-lock");
    let path = folder.path().join("history.jsonl");
    let lock = std::fs::File::create(path.with_extension("jsonl.lock")).unwrap();
    lock.lock().unwrap();
    let mut app = new_app();
    app.history_file = Some(path);
    handle_input(Event::Paste("hello".into()), &mut app, |_| {});
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        press(&mut app, KeyCode::Enter);
        let _ = sent.send(app.prompt_history.clone());
    });
    let history = received.recv_timeout(std::time::Duration::from_secs(2));
    drop(lock);
    assert_eq!(history, Ok(vec!["hello".to_string()]));
}
