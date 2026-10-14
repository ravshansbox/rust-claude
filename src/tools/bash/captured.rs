use crate::tools::MAX_OUTPUT;
use std::{collections::VecDeque, sync::Mutex};
use tokio::io::AsyncReadExt;

#[derive(Default)]
pub(super) struct Captured {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    pub(super) total: usize,
}

impl Captured {
    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        let room = MAX_OUTPUT.saturating_sub(self.head.len()).min(bytes.len());
        let (head, tail) = bytes.split_at(room);
        self.head.extend_from_slice(head);
        self.tail.extend(tail);
        let excess = self.tail.len().saturating_sub(MAX_OUTPUT);
        self.tail.drain(..excess);
    }

    /// The output in at most `limit` bytes: all of it when it fits, or else
    /// its start and end around a note of how much was left out.
    pub(super) fn text(self, limit: usize) -> String {
        let mut all = self.head;
        all.extend(self.tail);
        if self.total <= limit {
            return String::from_utf8(all)
                .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
        }
        let half = limit / 2;
        let mut start = &all[..half];
        if let Err(error) = std::str::from_utf8(start)
            && error.error_len().is_none()
        {
            start = &start[..error.valid_up_to()];
        }
        let mut end = &all[all.len() - half..];
        while end
            .first()
            .is_some_and(|byte| byte & 0b1100_0000 == 0b1000_0000)
        {
            end = &end[1..];
        }
        format!(
            "{}\n… {} bytes left out …\n{}",
            String::from_utf8_lossy(start),
            self.total - start.len() - end.len(),
            String::from_utf8_lossy(end)
        )
    }
}

pub(super) async fn read_capped(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    output: &Mutex<Captured>,
) {
    let mut chunk = [0; 8192];
    while let Ok(count) = reader.read(&mut chunk).await
        && count > 0
    {
        if let Ok(mut output) = output.lock() {
            output.push(&chunk[..count]);
        }
    }
}

fn take_captured(output: &Mutex<Captured>) -> Captured {
    output
        .lock()
        .map(|mut output| std::mem::take(&mut *output))
        .unwrap_or_default()
}

/// Both streams within `MAX_OUTPUT` bytes. Each gets half, and a stream
/// that needs less leaves the rest to the other.
pub(super) fn output_text(
    stdout_output: &Mutex<Captured>,
    stderr_output: &Mutex<Captured>,
) -> String {
    let (stdout, stderr) = (take_captured(stdout_output), take_captured(stderr_output));
    let half = MAX_OUTPUT / 2;
    let stdout_limit = MAX_OUTPUT - stderr.total.min(half);
    let stderr_limit = MAX_OUTPUT - stdout.total.min(stdout_limit);
    let mut text = stdout.text(stdout_limit);
    if stderr.total > 0 {
        text.push_str("\nstderr:\n");
        text.push_str(&stderr.text(stderr_limit));
    }
    text
}
