use pulldown_cmark::{Event, Options as ParseOptions, Parser, Tag};

pub(super) fn hard_line_breaks(text: &str) -> String {
    let mut text = text.to_string();
    let options = ParseOptions::ENABLE_STRIKETHROUGH
        | ParseOptions::ENABLE_TASKLISTS
        | ParseOptions::ENABLE_HEADING_ATTRIBUTES
        | ParseOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | ParseOptions::ENABLE_SUPERSCRIPT
        | ParseOptions::ENABLE_SUBSCRIPT
        | ParseOptions::ENABLE_MATH
        | ParseOptions::ENABLE_FOOTNOTES
        | ParseOptions::ENABLE_DEFINITION_LIST
        | ParseOptions::ENABLE_GFM
        | ParseOptions::ENABLE_TABLES;
    'parse: loop {
        let mut edits = Vec::new();
        for (event, range) in Parser::new_ext(&text, options).into_offset_iter() {
            match event {
                Event::Start(Tag::MetadataBlock(_)) => {
                    text.insert(range.start, '-');
                    continue 'parse;
                }
                Event::Start(Tag::Heading { .. }) => {
                    let heading = text[range.clone()].trim_end();
                    let end = range.start + heading.rfind('\n').unwrap_or(heading.len());
                    let line = text[..end].trim_end();
                    if line.ends_with('}') && !line.ends_with("\\}") {
                        edits.push((line.len() - 1, "\\"));
                    }
                }
                Event::SoftBreak => edits.push((range.start, "  ")),
                _ => {}
            }
        }
        edits.sort_unstable();
        for (offset, insert) in edits.into_iter().rev() {
            text.insert_str(offset, insert);
        }
        return text;
    }
}

/// The fence character, fence length and quote depth of the code block open
/// after `line`, given the one open before it.
pub(in crate::tui) fn next_fence(
    open: Option<(char, usize, usize)>,
    line: &str,
) -> Option<(char, usize, usize)> {
    let (trimmed, depth) = match open {
        Some((_, _, depth)) => match without_quotes(line, depth) {
            (rest, found) if found == depth => (rest.trim_start(), depth),
            _ => return next_fence(None, line),
        },
        None => {
            let (rest, depth) = without_quotes(line, usize::MAX);
            let (rest, inner) = without_quotes(without_list_marker(rest.trim_start()), usize::MAX);
            (rest.trim_start(), depth + inner)
        }
    };
    let Some((fence, length)) = fence_run(trimmed) else {
        return open;
    };
    if length < 3 {
        return open;
    }
    match open {
        None if fence == '`' && trimmed[length..].contains('`') => None,
        None => Some((fence, length, depth)),
        Some((open_fence, open_length, _))
            if fence == open_fence
                && length >= open_length
                && trimmed[length..].trim().is_empty() =>
        {
            None
        }
        _ => open,
    }
}

/// The fence character `trimmed` starts with, and how many times it repeats.
pub(in crate::tui) fn fence_run(trimmed: &str) -> Option<(char, usize)> {
    let fence = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    Some((fence, trimmed.chars().take_while(|c| *c == fence).count()))
}

/// `line` after up to `limit` blockquote markers, and how many it had.
fn without_quotes(line: &str, limit: usize) -> (&str, usize) {
    let mut rest = line;
    let mut depth = 0;
    while depth < limit
        && let Some(after) = rest.trim_start().strip_prefix('>')
    {
        rest = after;
        depth += 1;
    }
    (rest, depth)
}

/// `line` after a list item marker that starts it, such as `-` or `2.`.
fn without_list_marker(line: &str) -> &str {
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let marker = match line[digits..].chars().next() {
        Some('.' | ')') if (1..=9).contains(&digits) => digits + 1,
        Some('-' | '*' | '+') if digits == 0 => 1,
        _ => return line,
    };
    line[marker..]
        .strip_prefix([' ', '\t'])
        .map_or(line, str::trim_start)
}
