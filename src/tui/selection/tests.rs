use crate::tui::{
    App, Role,
    draw::draw,
    keys::handle_input,
    test_support::{app_with_reply, screen},
};
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::{Buffer, Cell},
    style::Modifier,
};

fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
    let event = Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
    handle_input(event, app, |_| {});
}

fn drag(app: &mut App, from: (u16, u16), to: (u16, u16)) -> String {
    let left = MouseButton::Left;
    mouse(app, MouseEventKind::Down(left), from.0, from.1);
    screen(app);
    mouse(app, MouseEventKind::Drag(left), to.0, to.1);
    screen(app);
    mouse(app, MouseEventKind::Up(left), to.0, to.1);
    app.input.clone()
}

/// Column and row where `text` starts on the screen.
fn position(app: &mut App, text: &str) -> (u16, u16) {
    let shown = screen(app);
    shown
        .lines()
        .enumerate()
        .find_map(|(row, line)| {
            line.find(text)
                .map(|index| (line[..index].chars().count() as u16, row as u16))
        })
        .unwrap_or_else(|| panic!("{text:?} not shown in\n{shown}"))
}

fn buffer(app: &mut App) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal.draw(|frame| draw(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn reversed_text(app: &mut App) -> String {
    let buffer = buffer(app);
    buffer
        .content()
        .iter()
        .filter(|cell| cell.modifier.contains(Modifier::REVERSED))
        .map(Cell::symbol)
        .collect()
}

#[test]
fn quotes_the_text_dragged_over_on_release() {
    let mut app = app_with_reply();
    let (column, row) = position(&mut app, "Earlier reply");
    let input = drag(&mut app, (column, row), (column + 6, row));
    assert_eq!(input, "'Earlier' - ");
    assert_eq!(app.cursor, input.len());
}

#[test]
fn quotes_a_selection_dragged_backwards() {
    let mut app = app_with_reply();
    let (column, row) = position(&mut app, "Earlier reply");
    let input = drag(&mut app, (column + 12, row), (column + 8, row));
    assert_eq!(input, "'reply' - ");
}

#[test]
fn quotes_each_row_on_its_own_line_without_trailing_spaces() {
    let mut app = app_with_reply();
    app.push(Role::Event, "second message");
    let (column, row) = position(&mut app, "reply");
    let (end_column, end_row) = position(&mut app, "second message");
    let input = drag(&mut app, (column, row), (end_column + 5, end_row));
    assert_eq!(input, "'reply\n\nsecond' - ");
}

#[test]
fn quotes_prompts_from_the_left_edge() {
    let mut app = app_with_reply();
    app.push(Role::User, "  my prompt");
    let (column, row) = position(&mut app, "Earlier reply");
    let (end_column, end_row) = position(&mut app, "  my prompt");
    let input = drag(&mut app, (column, row), (end_column + 10, end_row));
    assert_eq!(input, "'Earlier reply\n\n  my prompt' - ");
}

#[test]
fn keeps_the_leading_spaces_of_other_messages() {
    let mut app = app_with_reply();
    app.push(Role::Event, "    spaced");
    let (column, row) = position(&mut app, "    spaced");
    let input = drag(&mut app, (column, row), (column + 9, row));
    assert_eq!(input, "'    spaced' - ");
}

#[test]
fn quotes_at_the_cursor_after_a_space() {
    let mut app = app_with_reply();
    app.insert("before after");
    app.cursor = "before".len();
    let (column, row) = position(&mut app, "Earlier reply");
    let input = drag(&mut app, (column, row), (column + 6, row));
    assert_eq!(input, "before 'Earlier' -  after");
    assert_eq!(app.cursor, "before 'Earlier' - ".len());
}

#[test]
fn quotes_after_a_space_without_adding_another() {
    let mut app = app_with_reply();
    app.insert("before ");
    let (column, row) = position(&mut app, "Earlier reply");
    let input = drag(&mut app, (column, row), (column + 6, row));
    assert_eq!(input, "before 'Earlier' - ");
}

#[test]
fn highlights_the_selection_until_release() {
    let mut app = app_with_reply();
    let (column, row) = position(&mut app, "Earlier reply");
    let left = MouseButton::Left;
    mouse(&mut app, MouseEventKind::Down(left), column, row);
    assert_eq!(reversed_text(&mut app), "");
    mouse(&mut app, MouseEventKind::Drag(left), column + 6, row);
    assert_eq!(reversed_text(&mut app), "Earlier");
    mouse(&mut app, MouseEventKind::Up(left), column + 6, row);
    assert_eq!(reversed_text(&mut app), "");
}

#[test]
fn quotes_wide_characters_once() {
    let mut app = app_with_reply();
    app.push(Role::Event, "日本語 text");
    let (column, row) = position(&mut app, "text");
    let input = drag(&mut app, (column - 7, row), (column + 3, row));
    assert_eq!(input, "'日本語 text' - ");
}

#[test]
fn quotes_nothing_on_a_click() {
    let mut app = app_with_reply();
    let (column, row) = position(&mut app, "Earlier reply");
    let input = drag(&mut app, (column, row), (column, row));
    assert_eq!(input, "");
    assert_eq!(reversed_text(&mut app), "");
}

#[test]
fn keeps_the_selection_on_the_same_text_while_scrolling() {
    let mut app = app_with_reply();
    for number in 0..40 {
        app.push(Role::Event, format!("event {number}"));
    }
    let (column, row) = position(&mut app, "event 35");
    let left = MouseButton::Left;
    mouse(&mut app, MouseEventKind::Down(left), column, row);
    mouse(&mut app, MouseEventKind::Drag(left), column + 7, row);
    mouse(&mut app, MouseEventKind::ScrollUp, 0, 0);
    assert_ne!(position(&mut app, "event 35"), (column, row));
    assert_eq!(reversed_text(&mut app), "event 35");
}

#[test]
fn ignores_drags_that_start_outside_the_conversation() {
    let mut app = app_with_reply();
    let input = drag(&mut app, (0, 23), (0, 2));
    assert_eq!(input, "");
    assert_eq!(reversed_text(&mut app), "");
}

#[test]
fn drops_the_selection_when_the_terminal_is_resized() {
    let mut app = app_with_reply();
    let (column, row) = position(&mut app, "Earlier reply");
    let left = MouseButton::Left;
    mouse(&mut app, MouseEventKind::Down(left), column, row);
    mouse(&mut app, MouseEventKind::Drag(left), column + 6, row);
    handle_input(Event::Resize(100, 24), &mut app, |_| {});
    assert_eq!(reversed_text(&mut app), "");
}
