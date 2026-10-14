use super::super::render::{fence_run, next_fence};

/// Bytes of markdown up to the end of its last closed code block whose
/// opening fence starts the line, or up to its last whole line of plain
/// text that starts a block after a blank line. Nothing after it changes how
/// the text before it renders, except a link defined further down, which
/// shows once the reply finishes.
pub(super) fn finished_markdown_len(text: &str, paragraphs: bool) -> usize {
    let mut finished = 0;
    // The code block `next_fence` sees open, by its simpler rule.
    let mut block = None;
    // The fence character, length and indent of the open code block.
    let mut open: Option<(char, usize, usize)> = None;
    // HTML can hold blank lines, so after it only code blocks end a part.
    let mut html = false;
    // Fences inside an HTML block, which runs to a blank line, are its text.
    let mut in_html = false;
    let mut after_blank = false;
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        let start = end;
        end += line.len();
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        // A line indented less than the fence may end the list item it is in.
        if open.is_some_and(|(_, _, open_indent)| indent < open_indent) && !trimmed.is_empty() {
            return finished;
        }
        let in_fence = block.is_some();
        block = next_fence(block, line);
        if in_fence || open.is_some() {
            after_blank = false;
        } else {
            let starts_html = trimmed.starts_with('<') && line.len() - trimmed.len() <= 3;
            html |= starts_html;
            in_html = (in_html || starts_html) && !trimmed.is_empty();
            if paragraphs
                && after_blank
                && !html
                && line.ends_with('\n')
                && starts_plain_block(line)
            {
                finished = start;
            }
            after_blank = trimmed.is_empty();
        }
        let Some((fence, length)) = fence_run(trimmed) else {
            continue;
        };
        if length < 3 || in_html {
            continue;
        }
        match open {
            None if indent <= 3 && !(fence == '`' && trimmed[length..].contains('`')) => {
                open = Some((fence, length, indent));
            }
            Some((open_fence, open_length, open_indent))
                if indent <= 3
                    && fence == open_fence
                    && length >= open_length
                    && trimmed[length..].trim().is_empty() =>
            {
                open = None;
                if open_indent == 0 && block.is_none() && line.ends_with('\n') {
                    finished = end;
                }
            }
            _ => {}
        }
    }
    finished
}

/// Whether the line starts a block of its own at the left edge: not
/// indented, not a list item and not HTML.
fn starts_plain_block(line: &str) -> bool {
    let Some(first) = line.chars().next() else {
        return false;
    };
    let marker_end = |rest: &str| rest.is_empty() || rest.starts_with([' ', '\t', '\n']);
    if first.is_whitespace() || first == '<' {
        return false;
    }
    if matches!(first, '-' | '*' | '+') && marker_end(&line[1..]) {
        return false;
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    !(digits > 0 && line[digits..].starts_with(['.', ')']) && marker_end(&line[digits + 1..]))
}
