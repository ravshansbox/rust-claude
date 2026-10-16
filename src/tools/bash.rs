use super::{argument, number, tool};
use captured::{Captured, output_text, read_capped};
use futures::{
    FutureExt,
    future::{Either, FusedFuture},
};
use serde_json::{Value, json};
use std::{
    fmt::Write,
    pin::{Pin, pin},
    process::Stdio,
    sync::Mutex,
    time::Duration,
};
use tokio::{process::Command, time::Instant};

mod captured;

pub(super) fn definition() -> Value {
    tool(
        "bash",
        "Run a bash command in the cwd and return stdout and stderr",
        &json!({
            "command": { "type": "string" },
            "timeout": { "type": "integer", "description": "Seconds before the command is killed. No limit when omitted" }
        }),
        &["command"],
    )
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let timeout = number(input, "timeout", 1)?;
    let mut command = Command::new("bash");
    command
        .args(["-c", argument(input, "command")?])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    new_session(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run command: {error}"))?;
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
        if !text.is_empty() {
            text.push('\n');
        }
        let _ = write!(text, "{status}");
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

#[cfg(test)]
mod tests;
