use super::{ChatMessage, Role};
use ratatui::text::Line;

#[test]
fn rerenders_only_width_dependent_messages_on_resize() {
    let messages = [
        (
            Role::Tool,
            "edit src/main.rs\n-fn old() {}\n+fn new() { println!(\"a line long enough to wrap\"); }",
            true,
        ),
        (Role::Event, "an event line long enough to wrap", false),
        (Role::Thinking, "thinking about something long", false),
        (
            Role::Assistant,
            "Some text\n\n```rust\nfn main() { println!(\"hi there\"); }\n```\n",
            false,
        ),
        (Role::User, "a prompt", true),
        (
            Role::Assistant,
            "Table:\n\n| a | b |\n| --- | --- |\n| one two three four | five six seven |\n",
            true,
        ),
        (
            Role::Assistant,
            "> | a | b |\n> |:-:|---|\n> | one two three four | five six seven |\n",
            true,
        ),
    ];
    for (role, text, depends_on_width) in messages {
        let message = || ChatMessage {
            role,
            text: text.into(),
            rendered: None,
            streaming: false,
        };
        let mut resized = message();
        resized.render(60);
        resized.render(20);
        let mut fresh = message();
        fresh.render(20);
        let (resized, fresh) = (resized.rendered.unwrap(), fresh.rendered.unwrap());
        assert_eq!(resized.lines, fresh.lines, "{text}");
        assert_eq!(resized.heights, fresh.heights, "{text}");
        assert_eq!(resized.height, fresh.height, "{text}");

        let mut marked = message();
        marked.render(60);
        marked.rendered.as_mut().unwrap().lines[0] = Line::raw("cached");
        marked.render(20);
        let rendered = marked.rendered.unwrap();
        let kept = rendered.lines[0] == Line::raw("cached");
        assert_eq!(kept, !depends_on_width, "{text}");
    }
}

#[test]
fn renders_a_code_fence_inside_html_as_part_of_it() {
    let text = "<div>\n```\ncode\n```\n</div>\n\nAfter\n";
    let mut message = ChatMessage {
        role: Role::Assistant,
        text: text.into(),
        rendered: None,
        streaming: false,
    };
    message.render(80);
    let lines: Vec<String> = message
        .rendered
        .unwrap()
        .lines
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        lines,
        ["<div>  ", "```", "code", "```", "</div>  ", "", "After"]
    );
}
