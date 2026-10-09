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

fn text_width(text: &str) -> usize {
    text.chars()
        .map(|character| character.width().unwrap_or(0))
        .sum()
}

fn index_at_width(line: &str, column: usize) -> usize {
    let mut width = 0;
    for (index, character) in line.char_indices() {
        let character_width = character.width().unwrap_or(0);
        if width + character_width > column {
            return index;
        }
        width += character_width;
    }
    line.len()
}

pub(super) fn line_above(input: &str, cursor: usize) -> Option<usize> {
    let line_start = input[..cursor].rfind('\n')? + 1;
    let previous_end = line_start - 1;
    let previous_start = input[..previous_end]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let column = text_width(&input[line_start..cursor]);
    Some(previous_start + index_at_width(&input[previous_start..previous_end], column))
}

pub(super) fn line_below(input: &str, cursor: usize) -> Option<usize> {
    let line_start = input[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let next_start = cursor + input[cursor..].find('\n')? + 1;
    let next_end = input[next_start..]
        .find('\n')
        .map_or(input.len(), |index| next_start + index);
    let column = text_width(&input[line_start..cursor]);
    Some(next_start + index_at_width(&input[next_start..next_end], column))
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
        assert_eq!(line_above(input, 2), None);
        assert_eq!(line_above(input, "abcd\n你".len()), Some(2));
        assert_eq!(line_above(input, input.len()), Some("abcd\n你".len()));
        assert_eq!(line_below(input, 4), Some("abcd\n你好".len()));
        assert_eq!(
            line_below(input, "abcd\n你".len()),
            Some("abcd\n你好\nxy".len())
        );
        assert_eq!(line_below(input, input.len()), None);
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
