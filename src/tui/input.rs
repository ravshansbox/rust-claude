use unicode_width::UnicodeWidthChar;

pub(super) fn input_rows(input: &str, width: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut row_width = 0;
    for character in input.chars() {
        if character == '\n' {
            rows.push(String::new());
            row_width = 0;
            continue;
        }
        let character_width = character.width().unwrap_or(0);
        if row_width > 0 && row_width + character_width > width {
            rows.push(String::new());
            row_width = 0;
        }
        if let Some(row) = rows.last_mut() {
            row.push(character);
        }
        row_width += character_width;
    }
    if row_width >= width {
        rows.push(String::new());
    }
    rows
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

pub(super) fn previous_word_start(input: &str, cursor: usize) -> usize {
    let mut characters = input[..cursor].char_indices().rev().peekable();
    while characters
        .next_if(|(_, character)| !is_word_character(*character))
        .is_some()
    {}
    let mut start = characters.peek().map_or(0, |(index, _)| *index);
    while let Some((index, _)) = characters.next_if(|(_, character)| is_word_character(*character))
    {
        start = index;
    }
    start
}

pub(super) fn next_word_end(input: &str, cursor: usize) -> usize {
    let mut characters = input[cursor..].char_indices().peekable();
    while characters
        .next_if(|(_, character)| !is_word_character(*character))
        .is_some()
    {}
    while characters
        .next_if(|(_, character)| is_word_character(*character))
        .is_some()
    {}
    characters
        .peek()
        .map_or(input.len(), |(index, _)| cursor + index)
}

pub(super) fn input_cursor(input: &str, cursor: usize, width: usize) -> (usize, usize) {
    let mut row = 0;
    let mut row_width = 0;
    for (index, character) in input.char_indices() {
        let character_width = character.width().unwrap_or(0);
        if character != '\n' && row_width > 0 && row_width + character_width > width {
            row += 1;
            row_width = 0;
        }
        if index == cursor {
            return (row, row_width);
        }
        if character == '\n' {
            row += 1;
            row_width = 0;
        } else {
            row_width += character_width;
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
    for (index, character) in input.char_indices() {
        let character_width = character.width().unwrap_or(0);
        if character != '\n' && row_width > 0 && row_width + character_width > width {
            if row == target_row {
                return last_index;
            }
            row += 1;
            row_width = 0;
        }
        if row == target_row && (character == '\n' || row_width + character_width > column) {
            return index;
        }
        if character == '\n' {
            row += 1;
            row_width = 0;
        } else {
            row_width += character_width;
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
