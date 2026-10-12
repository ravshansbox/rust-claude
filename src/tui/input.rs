use ratatui::buffer::CellWidth;
use unicode_segmentation::UnicodeSegmentation;

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

/// Width as ratatui draws it, which gives halfwidth katakana sound marks
/// their own cell.
fn shown_width(grapheme: &str) -> usize {
    usize::from(shown(grapheme).cell_width())
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
    if matches!(grapheme, "\n" | "") {
        row_width >= width
    } else {
        row_width > 0 && row_width + grapheme_width.max(1) > width
    }
}

fn layout(input: &str, width: usize) -> impl Iterator<Item = (usize, &str, usize, usize)> {
    let (mut row, mut row_width) = (0, 0);
    input
        .grapheme_indices(true)
        .chain([(input.len(), "")])
        .map(move |(index, grapheme)| {
            let grapheme_width = shown_width(grapheme);
            if starts_new_row(grapheme, grapheme_width, row_width, width) {
                row += 1;
                row_width = 0;
            }
            let position = (index, grapheme, row, row_width);
            if grapheme == "\n" {
                row += 1;
                row_width = 0;
            } else {
                row_width += grapheme_width;
            }
            position
        })
}

pub(super) fn input_rows(input: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    for (_, grapheme, row, _) in layout(input, width) {
        rows.resize(rows.len().max(row + 1), String::new());
        rows[row].push_str(shown(grapheme));
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
    layout(input, width)
        .find(|&(index, ..)| index == cursor || index == input.len())
        .map_or((0, 0), |(_, _, row, column)| (row, column))
}

fn index_at(input: &str, target_row: usize, column: usize, width: usize) -> usize {
    let mut last_index = 0;
    for (index, grapheme, row, row_width) in layout(input, width) {
        if grapheme.is_empty() {
            break;
        }
        if row > target_row {
            return last_index;
        }
        if row == target_row && (grapheme == "\n" || row_width + shown_width(grapheme) > column) {
            return index;
        }
        last_index = index;
    }
    input.len()
}

pub(super) fn row_above(input: &str, cursor: usize, width: usize) -> Option<usize> {
    let (row, column) = input_cursor(input, cursor, width);
    row.checked_sub(1)
        .map(|target_row| index_at(input, target_row, column, width))
}

pub(super) fn row_below(input: &str, cursor: usize, width: usize) -> Option<usize> {
    let (row, column) = input_cursor(input, cursor, width);
    let (.., last_row, _) = layout(input, width).last()?;
    (row < last_row).then(|| index_at(input, row + 1, column, width))
}

#[cfg(test)]
mod tests;
