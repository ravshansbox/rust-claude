use super::{MAX_OUTPUT, Output, argument, failed, number, plural, regular_file, tool, truncate};
use crate::images;
use serde_json::{Value, json};
use std::{
    fmt::Write,
    fs::File,
    io::{self, BufRead, BufReader, ErrorKind, Read},
    path::PathBuf,
};

pub(super) fn definition() -> Value {
    tool(
        "read",
        "Read a UTF-8 file or an image (PNG, JPEG, GIF, WebP or TIFF) from the cwd. Images are sent as attachments, scaled to fit 2000x2000. Long text output is cut at whole lines; use offset and limit to read the rest",
        &json!({
            "path": { "type": "string" },
            "offset": { "type": "integer", "description": "Line number to start from, counting from 1" },
            "limit": { "type": "integer", "description": "Maximum number of lines to read" }
        }),
        &["path"],
    )
}

pub(super) async fn run(input: &Value) -> Result<Output, String> {
    let path = argument(input, "path")?;
    let offset = number(input, "offset", 0)?.map_or(1, |offset| {
        usize::try_from(offset.max(1)).unwrap_or(usize::MAX)
    });
    let limit = number(input, "limit", 1)?.map_or(usize::MAX, |limit| {
        usize::try_from(limit).unwrap_or(usize::MAX)
    });
    regular_file(path).await?;
    let file = PathBuf::from(path);
    tokio::task::spawn_blocking(move || {
        let mut reader = BufReader::new(File::open(&file)?);
        if is_image(reader.fill_buf()?) {
            let data = std::fs::read(&file)?;
            return Ok(images::prepare(data)
                .map(Output::Image)
                .map_err(|error| format!("{error:#}")));
        }
        Ok(read_lines(reader, offset, limit)?
            .map(Output::Text)
            .map_err(|line_count| {
                format!(
                    "offset {offset} is past the end of {}, which has {}",
                    file.display(),
                    plural(line_count, "line")
                )
            }))
    })
    .await
    .map_err(io::Error::other)
    .flatten()
    .map_err(failed("read", path))?
}

/// Whether `header` starts like an image this build can decode. Formats it
/// cannot decode are left out, since their headers, such as BM, can also
/// start text.
fn is_image(header: &[u8]) -> bool {
    image::guess_format(header).is_ok_and(|format| format.reading_enabled())
}

/// Reads `limit` lines from line `offset`, stopping once the output is full,
/// so only the part of the file that is shown gets loaded. Returns the number
/// of lines in the file when `offset` is past its end.
fn read_lines(
    mut reader: impl BufRead,
    offset: usize,
    limit: usize,
) -> io::Result<Result<String, usize>> {
    let mut skipped = 0;
    while skipped < offset - 1 && reader.skip_until(b'\n')? > 0 {
        skipped += 1;
    }
    let mut text = String::new();
    let mut line = Vec::new();
    for index in (offset - 1..).take(limit) {
        line.clear();
        // A few bytes more than fit, so a cut line still ends in whole
        // characters past the cut.
        let room = MAX_OUTPUT - text.len();
        (&mut reader)
            .take(u64::try_from(room + 4).unwrap_or(u64::MAX))
            .read_until(b'\n', &mut line)?;
        if line.is_empty() {
            if index == offset - 1 && offset > 1 {
                return Ok(Err(skipped));
            }
            break;
        }
        if line.len() > room {
            let next_line = if text.is_empty() {
                text = truncate(utf8_prefix(&line)?.to_string());
                index + 2
            } else {
                text.push_str("… output truncated");
                index + 1
            };
            let _ = write!(text, ", continue with offset {next_line}");
            return Ok(Ok(text));
        }
        text.push_str(std::str::from_utf8(&line).map_err(|_| not_utf8())?);
    }
    Ok(Ok(text))
}

/// The text in `bytes`, which may end partway through a character.
fn utf8_prefix(bytes: &[u8]) -> io::Result<&str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Ok(text),
        Err(error) if error.error_len().is_none() => utf8_prefix(&bytes[..error.valid_up_to()]),
        Err(_) => Err(not_utf8()),
    }
}

fn not_utf8() -> io::Error {
    io::Error::new(ErrorKind::InvalidData, "stream did not contain valid UTF-8")
}

#[cfg(test)]
mod tests;
