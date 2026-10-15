use crate::tui::{
    Role,
    keys::handle_input,
    test_support::{app_with_reply, new_app, press, screen, screen_of_width},
};
use crossterm::event::{Event, KeyCode};
use ratatui::{Terminal, backend::TestBackend};

mod lists;
mod scrolling;

#[test]
fn shows_the_input_without_a_marker() {
    let mut app = app_with_reply();
    handle_input(Event::Paste("lorem ipsum dolor".into()), &mut app, |_| {});
    let shown = screen_of_width(&mut app, 10);
    let rows: Vec<&str> = shown.lines().collect();
    let first = rows
        .iter()
        .position(|row| row.starts_with("lorem"))
        .unwrap_or_else(|| panic!("{shown}"));
    assert_eq!(rows[first], "lorem ipsu", "{shown}");
    assert_eq!(rows[first + 1], "m dolor   ", "{shown}");
    let mut terminal = Terminal::new(TestBackend::new(10, 24)).unwrap();
    terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
    let cursor = terminal.get_cursor_position().unwrap();
    assert_eq!((cursor.x, cursor.y), (7, u16::try_from(first).unwrap() + 1));
    press(&mut app, KeyCode::Up);
    assert_eq!(app.cursor, "lorem i".len());
}

fn rows_from<'a>(shown: &'a str, start: &str) -> Vec<&'a str> {
    let rows: Vec<&str> = shown.lines().map(str::trim_end).collect();
    let first = rows
        .iter()
        .position(|row| row.starts_with(start))
        .unwrap_or_else(|| panic!("{shown}"));
    rows[first..].to_vec()
}

#[test]
fn shows_notices_in_a_row_without_blank_rows_between_them() {
    let mut app = new_app();
    app.push(Role::Event, "first notice");
    app.push(Role::Event, "second notice");
    app.push(Role::Assistant, "reply");
    app.push(Role::Event, "last notice");
    let shown = screen(&mut app);
    let rows: Vec<&str> = shown.lines().map(str::trim_end).collect();
    assert!(rows[0].starts_with("Rust Coding Agent v"), "{shown}");
    assert_eq!(
        rows[1..7],
        [
            "first notice",
            "second notice",
            "",
            "reply",
            "",
            "last notice"
        ],
        "{shown}"
    );
}

#[test]
fn shows_a_reply_at_the_left_edge() {
    let mut app = new_app();
    app.push(Role::Assistant, "alpha beta gamma delta\n\nsecond part");
    let shown = screen_of_width(&mut app, 14);
    let rows = rows_from(&shown, "alpha");
    assert_eq!(
        rows[..4],
        ["alpha beta", "gamma delta", "", "second part"],
        "{shown}"
    );
}

#[test]
fn shows_the_same_rows_from_any_scroll_position() {
    use ratatui::{
        buffer::{Buffer, Cell},
        layout::Rect,
        widgets::Widget,
    };
    let mut app = new_app();
    let words: String = (0..30).map(|number| format!("word{number:02} ")).collect();
    app.push(Role::Event, "first event");
    app.push(Role::Assistant, format!("{words}\n\n{words}\n\nend"));
    app.push(Role::Event, format!("{words}last"));
    let width = 20;
    screen_of_width(&mut app, width);
    let rows = |top: usize, height: u16| {
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        super::conversation(&app, width, top, height).render(area, &mut buffer);
        buffer
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
    };
    let all = rows(0, 200);
    let total = all.iter().rposition(|row| !row.trim().is_empty()).unwrap() + 1;
    assert!(total > 35, "{all:?}");
    for top in 0..total {
        let end = (top + 5).min(all.len());
        assert_eq!(rows(top, 5)[..end - top], all[top..end], "from row {top}");
    }
}
