use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const TAB: &str = "    ";

/// The text drawn for a grapheme other than a line break. ratatui skips
/// control characters, so a tab shows as spaces and the rest show as nothing.
fn shown(grapheme: &str) -> &str {
    if grapheme == "\t" {
        TAB
    } else if grapheme.contains(char::is_control) {
        ""
    } else {
        grapheme
    }
}

fn shown_width(grapheme: &str) -> usize {
    shown(grapheme).width()
}

/// Text with each tab shown as spaces, as in the input box, since ratatui
/// skips control characters.
pub(super) fn expand_tabs(text: String) -> String {
    if text.contains('\t') {
        text.replace('\t', TAB)
    } else {
        text
    }
}

fn starts_new_row(grapheme: &str, grapheme_width: usize, row_width: usize, width: usize) -> bool {
    if grapheme == "\n" {
        row_width >= width
    } else {
        row_width > 0 && row_width + grapheme_width > width
    }
}

pub(super) fn input_rows(input: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut row_width = 0;
    for grapheme in input.graphemes(true) {
        let grapheme_width = shown_width(grapheme);
        if starts_new_row(grapheme, grapheme_width, row_width, width) {
            rows.push(String::new());
            row_width = 0;
        }
        if grapheme == "\n" {
            rows.push(String::new());
            row_width = 0;
            continue;
        }
        if let Some(row) = rows.last_mut() {
            row.push_str(shown(grapheme));
        }
        row_width += grapheme_width;
    }
    if row_width >= width {
        rows.push(String::new());
    }
    rows
}

pub(super) fn previous_grapheme(input: &str, cursor: usize) -> usize {
    input[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(index, _)| index)
}

pub(super) fn next_grapheme(input: &str, cursor: usize) -> usize {
    input[cursor..]
        .graphemes(true)
        .next()
        .map_or(cursor, |grapheme| cursor + grapheme.len())
}

fn is_word(grapheme: &str) -> bool {
    grapheme
        .chars()
        .next()
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
}

pub(super) fn previous_word_start(input: &str, cursor: usize) -> usize {
    let mut graphemes = input[..cursor].grapheme_indices(true).rev().peekable();
    while graphemes
        .next_if(|(_, grapheme)| !is_word(grapheme))
        .is_some()
    {}
    let mut start = graphemes.peek().map_or(0, |(index, _)| *index);
    while let Some((index, _)) = graphemes.next_if(|(_, grapheme)| is_word(grapheme)) {
        start = index;
    }
    start
}

pub(super) fn next_word_end(input: &str, cursor: usize) -> usize {
    let mut graphemes = input[cursor..].grapheme_indices(true).peekable();
    while graphemes
        .next_if(|(_, grapheme)| !is_word(grapheme))
        .is_some()
    {}
    while graphemes
        .next_if(|(_, grapheme)| is_word(grapheme))
        .is_some()
    {}
    graphemes
        .peek()
        .map_or(input.len(), |(index, _)| cursor + index)
}

pub(super) fn input_cursor(input: &str, cursor: usize, width: usize) -> (usize, usize) {
    let mut row = 0;
    let mut row_width = 0;
    for (index, grapheme) in input.grapheme_indices(true) {
        let grapheme_width = shown_width(grapheme);
        if starts_new_row(grapheme, grapheme_width, row_width, width) {
            row += 1;
            row_width = 0;
        }
        if index == cursor {
            return (row, row_width);
        }
        if grapheme == "\n" {
            row += 1;
            row_width = 0;
        } else {
            row_width += grapheme_width;
        }
    }
    if row_width >= width {
        (row + 1, 0)
    } else {
        (row, row_width)
    }
}

fn index_at(input: &str, target_row: usize, column: usize, width: usize) -> usize {
    let mut row = 0;
    let mut row_width = 0;
    let mut last_index = 0;
    for (index, grapheme) in input.grapheme_indices(true) {
        let grapheme_width = shown_width(grapheme);
        if starts_new_row(grapheme, grapheme_width, row_width, width) {
            if row == target_row {
                return last_index;
            }
            row += 1;
            row_width = 0;
        }
        if row == target_row && (grapheme == "\n" || row_width + grapheme_width > column) {
            return index;
        }
        if grapheme == "\n" {
            row += 1;
            row_width = 0;
        } else {
            row_width += grapheme_width;
        }
        last_index = index;
    }
    input.len()
}

pub(super) fn row_above(input: &str, cursor: usize, width: usize) -> Option<usize> {
    let (row, column) = input_cursor(input, cursor, width);
    let target_row = row.checked_sub(1)?;
    Some(index_at(input, target_row, column, width))
}

pub(super) fn row_below(input: &str, cursor: usize, width: usize) -> Option<usize> {
    let (row, column) = input_cursor(input, cursor, width);
    if row + 1 >= input_rows(input, width).len() {
        return None;
    }
    Some(index_at(input, row + 1, column, width))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measures_emoji_as_whole_graphemes() {
        assert_eq!(input_cursor("👍🏽 ok", "👍🏽 ok".len(), 10), (0, 5));
        assert_eq!(input_cursor("❤️👨‍👩‍👧x", "❤️👨‍👩‍👧".len(), 10), (0, 4));
        assert_eq!(input_rows("❤️❤️❤️", 4), vec!["❤️❤️", "❤️"]);
        assert_eq!(row_below("❤️❤️❤️", "❤️".len(), 4), Some("❤️❤️❤️".len()));
        assert_eq!(row_above("❤️❤️❤️", "❤️❤️❤️".len(), 4), Some("❤️".len()));
    }

    #[test]
    fn wraps_input_by_display_width() {
        assert_eq!(input_rows("你好世界", 5), vec!["你好", "世界"]);
        assert_eq!(input_rows("abcd", 4), vec!["abcd", ""]);
        assert_eq!(input_rows("", 4), vec![""]);
        assert_eq!(input_rows("ab\ncd\n", 4), vec!["ab", "cd", ""]);
    }

    #[test]
    fn places_cursor_by_display_width() {
        assert_eq!(input_cursor("你好世界", 3, 5), (0, 2));
        assert_eq!(input_cursor("你好世界", 6, 5), (1, 0));
        assert_eq!(input_cursor("你好世界", 12, 5), (1, 4));
        assert_eq!(input_cursor("abcd", 4, 4), (1, 0));
        assert_eq!(input_cursor("ab\ncd", 2, 4), (0, 2));
        assert_eq!(input_cursor("ab\ncd", 3, 4), (1, 0));
        assert_eq!(input_cursor("", 0, 4), (0, 0));
    }

    #[test]
    fn moves_between_lines_by_display_width() {
        let input = "abcd\n你好\nxy";
        let width = usize::MAX;
        assert_eq!(row_above(input, 2, width), None);
        assert_eq!(row_above(input, "abcd\n你".len(), width), Some(2));
        assert_eq!(row_above(input, input.len(), width), Some("abcd\n你".len()));
        assert_eq!(row_below(input, 4, width), Some("abcd\n你好".len()));
        assert_eq!(
            row_below(input, "abcd\n你".len(), width),
            Some("abcd\n你好\nxy".len())
        );
        assert_eq!(row_below(input, input.len(), width), None);
    }

    #[test]
    fn moves_between_wrapped_rows() {
        assert_eq!(row_above("abcdefg", 6, 4), Some(2));
        assert_eq!(row_below("abcdefg", 1, 4), Some(5));
        assert_eq!(row_below("abcdefg", 3, 4), Some(7));
        assert_eq!(row_above("abcdefg", 2, 4), None);
        assert_eq!(row_below("abcdefg", 5, 4), None);
        assert_eq!(row_above("abc你d", 6, 4), Some(2));
        assert_eq!(row_below("abc你d", 2, 4), Some(6));
        assert_eq!(row_above("ab你cdef", 5, 3), Some(1));
        assert_eq!(row_below("abcd", 0, 4), Some(4));
    }

    #[test]
    fn moves_newline_after_full_row_to_next_row() {
        assert_eq!(input_rows("abcd\nx", 4), vec!["abcd", "", "x"]);
        assert_eq!(input_cursor("abcd\nx", 4, 4), (1, 0));
        assert_eq!(input_cursor("abcd\nx", 5, 4), (2, 0));
        assert_eq!(row_below("abcd\nx", 4, 4), Some(5));
        assert_eq!(row_above("abcd\nx", 5, 4), Some(4));
    }

    #[test]
    fn lays_out_tabs_and_control_characters_as_drawn() {
        assert_eq!(input_rows("\tif x {", 20), vec!["    if x {"]);
        assert_eq!(input_rows("\t\tab", 7), vec!["    ", "    ab"]);
        assert_eq!(input_cursor("\tab", 1, 20), (0, 4));
        assert_eq!(input_cursor("\u{1b}ab", 3, 20), (0, 2));
        assert_eq!(input_rows("\u{1b}ab", 20), vec!["ab"]);
        assert_eq!(row_above("\tab\nxyzwv", "\tab\nxyzwv".len(), 20), Some(2));
        assert_eq!(row_below("ab\n\tcd", 1, 20), Some(3));
    }

    #[test]
    fn finds_word_boundaries() {
        let input = "fn  snake_case(héllo) ";
        assert_eq!(
            previous_word_start(input, input.len()),
            "fn  snake_case(".len()
        );
        assert_eq!(
            previous_word_start(input, "fn  snake_case(".len()),
            "fn  ".len()
        );
        assert_eq!(previous_word_start(input, "fn  sna".len()), "fn  ".len());
        assert_eq!(previous_word_start(input, "fn  ".len()), 0);
        assert_eq!(previous_word_start(input, 0), 0);
        assert_eq!(next_word_end(input, 0), "fn".len());
        assert_eq!(next_word_end(input, "fn".len()), "fn  snake_case".len());
        assert_eq!(
            next_word_end(input, "fn  snake_case".len()),
            "fn  snake_case(héllo".len()
        );
        assert_eq!(
            next_word_end(input, "fn  snake_case(héllo".len()),
            input.len()
        );
    }
}
