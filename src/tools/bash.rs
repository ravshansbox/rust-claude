use super::{MAX_OUTPUT, argument, number, tool};
use futures::{
    FutureExt,
    future::{Either, FusedFuture},
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fmt::Write,
    pin::{Pin, pin},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    time::Instant,
};

pub(super) fn definition() -> Value {
    tool(
        "bash",
        "Run a bash command in the cwd and return stdout and stderr",
        &json!({
            "command": { "type": "string" },
            "timeout": { "type": "integer", "description": "Seconds before the command is killed. No limit when omitted" },
            "run_in_background": { "type": "boolean", "description": "Return at once and give the output in a later message when the command finishes, for long waits such as CI runs" }
        }),
        &["command"],
    )
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let (child, timeout) = start(input)?;
    wait(child, timeout).await
}

/// Starts the command of a `bash` call, and gives it with its timeout.
pub(super) fn start(input: &Value) -> Result<(Child, Option<u64>), String> {
    let timeout = number(input, "timeout", 1)?;
    let mut command = Command::new("bash");
    command
        .args(["-c", argument(input, "command")?])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    new_session(&mut command);
    let child = command
        .spawn()
        .map_err(|error| format!("failed to run command: {error}"))?;
    Ok((child, timeout))
}

/// Waits for a started command and gives its output.
pub(super) async fn wait(mut child: Child, timeout: Option<u64>) -> Result<String, String> {
    let mut process_group = ProcessGroup(child.id());
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err("failed to capture command output".into());
    };
    let stdout_output = Mutex::new(Captured::default());
    let stderr_output = Mutex::new(Captured::default());
    // Read in this task rather than in tasks of their own, so output already
    // in the pipes when the command exits is read before the grace runs out,
    // however busy the runtime is.
    let mut reading = pin!(
        futures::future::join(
            read_capped(stdout, &stdout_output),
            read_capped(stderr, &stderr_output),
        )
        .fuse()
    );
    let run = async {
        match futures::future::select(pin!(child.wait()), reading.as_mut()).await {
            Either::Left((status, _)) => status,
            Either::Right((_, run)) => run.await,
        }
    };
    let status = match timeout {
        Some(seconds) => tokio::time::timeout(Duration::from_secs(seconds), run)
            .await
            .map_err(|_| seconds),
        None => Ok(run.await),
    };
    match status {
        Ok(_) => process_group.0 = None,
        Err(_) => drop(process_group),
    }
    finish_reading(reading, [&stdout_output, &stderr_output]).await;
    let status = match status {
        Ok(status) => status.map_err(|error| format!("failed to run command: {error}"))?,
        Err(seconds) => {
            let text = output_text(&stdout_output, &stderr_output);
            let notice = format!("command timed out after {seconds}s");
            return Err(if text.is_empty() {
                notice
            } else {
                format!("{text}\n{notice}")
            });
        }
    };
    let mut text = output_text(&stdout_output, &stderr_output);
    if !status.success() {
        let _ = write!(text, "\n{status}");
    }
    Ok(text)
}

/// How long output may pause after the command exits before the rest is left
/// out, and how long to wait for it in all. A process the command started in
/// the background can keep the pipes open long after it exits.
const OUTPUT_GRACE: Duration = Duration::from_millis(100);
const OUTPUT_GRACE_LIMIT: Duration = Duration::from_secs(1);

/// Reads the output still to come after the command exits, until the pipes
/// close or no more arrives within `OUTPUT_GRACE`.
async fn finish_reading(mut reading: Pin<&mut impl FusedFuture>, outputs: [&Mutex<Captured>; 2]) {
    let limit = Instant::now() + OUTPUT_GRACE_LIMIT;
    let mut received = bytes_received(outputs);
    while !reading.is_terminated() {
        let pause = (Instant::now() + OUTPUT_GRACE).min(limit);
        if tokio::time::timeout_at(pause, reading.as_mut())
            .await
            .is_ok()
        {
            return;
        }
        let now = bytes_received(outputs);
        if now == received || Instant::now() >= limit {
            return;
        }
        received = now;
    }
}

fn bytes_received(outputs: [&Mutex<Captured>; 2]) -> usize {
    outputs
        .iter()
        .map(|output| output.lock().map_or(0, |output| output.total))
        .sum()
}

/// Starts the command in a session of its own, without the terminal, so prompts
/// that open /dev/tty fail at once instead of stopping it, and its process group
/// can be killed as a whole.
pub fn new_session(command: &mut Command) {
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

pub struct ProcessGroup(pub Option<u32>);

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(id) = self.0 {
            unsafe {
                libc::killpg(id as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}

#[derive(Default)]
struct Captured {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    total: usize,
}

impl Captured {
    fn push(&mut self, bytes: &[u8]) {
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
    fn text(self, limit: usize) -> String {
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

async fn read_capped(mut reader: impl tokio::io::AsyncRead + Unpin, output: &Mutex<Captured>) {
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
fn output_text(stdout_output: &Mutex<Captured>, stderr_output: &Mutex<Captured>) -> String {
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

#[cfg(test)]
mod tests;
