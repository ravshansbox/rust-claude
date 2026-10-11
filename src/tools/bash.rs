use super::{MAX_OUTPUT, argument, number};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Child};

pub(super) fn definition() -> Value {
    json!({
        "name": "bash",
        "description": "Run a bash command in the cwd and return stdout and stderr",
        "input_schema": {
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeout": { "type": "integer", "description": "Seconds before the command is killed. No limit when omitted" },
                "run_in_background": { "type": "boolean", "description": "Return at once and give the output in a later message when the command finishes, for long waits such as CI runs" }
            },
            "required": ["command"]
        }
    })
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let (child, timeout) = start(input)?;
    wait(child, timeout).await
}

/// Starts the command of a `bash` call, and gives it with its timeout.
pub(super) fn start(input: &Value) -> Result<(Child, Option<u64>), String> {
    let timeout = number(input, "timeout", 1)?;
    let mut command = tokio::process::Command::new("bash");
    command.args(["-c", argument(input, "command")?]);
    command
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
    let stdout_output = Arc::new(Mutex::new(Captured::default()));
    let stderr_output = Arc::new(Mutex::new(Captured::default()));
    let readers = [
        tokio::spawn(read_capped(stdout, stdout_output.clone())),
        tokio::spawn(read_capped(stderr, stderr_output.clone())),
    ];
    let run = child.wait();
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
    let _ = tokio::time::timeout(OUTPUT_GRACE, futures::future::join_all(readers)).await;
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
        text.push_str(&format!("\n{status}"));
    }
    Ok(text)
}

const OUTPUT_GRACE: Duration = Duration::from_millis(100);

/// Starts the command in a session of its own, without the terminal, so prompts
/// that open /dev/tty fail at once instead of stopping it, and its process group
/// can be killed as a whole.
pub fn new_session(command: &mut tokio::process::Command) {
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

#[derive(Clone, Default)]
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
    fn text(&self, limit: usize) -> String {
        let all: Vec<u8> = self.head.iter().chain(&self.tail).copied().collect();
        if self.total <= limit {
            return String::from_utf8_lossy(&all).into_owned();
        }
        let half = limit / 2;
        let mut start = &all[..half];
        while let Err(error) = std::str::from_utf8(start)
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

async fn read_capped(mut reader: impl tokio::io::AsyncRead + Unpin, output: Arc<Mutex<Captured>>) {
    let mut chunk = [0; 8192];
    while let Ok(count) = reader.read(&mut chunk).await
        && count > 0
    {
        if let Ok(mut output) = output.lock() {
            output.push(&chunk[..count]);
        }
    }
}

fn snapshot(output: &Mutex<Captured>) -> Captured {
    output
        .lock()
        .map(|output| output.clone())
        .unwrap_or_default()
}

/// Both streams within `MAX_OUTPUT` bytes. Each gets half, and a stream
/// that needs less leaves the rest to the other.
fn output_text(stdout_output: &Mutex<Captured>, stderr_output: &Mutex<Captured>) -> String {
    let (stdout, stderr) = (snapshot(stdout_output), snapshot(stderr_output));
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
mod tests {
    use crate::tools::{MAX_OUTPUT, call, test_support::TemporaryFile};
    use serde_json::json;

    #[tokio::test]
    async fn times_out_bash_command() {
        let input = json!({ "command": "sleep 5", "timeout": 1 });
        assert_eq!(
            call("bash", &input).await,
            Err("command timed out after 1s".into())
        );
    }

    #[tokio::test]
    async fn runs_bash_command_without_the_terminal() {
        let command = "ps -o stat= -p $$ | grep -q s && echo own session; \
                       (exec 3</dev/tty) 2>/dev/null || echo no terminal";
        let input = json!({ "command": command, "timeout": 5 });
        assert_eq!(
            call("bash", &input).await,
            Ok("own session\nno terminal\n".into())
        );
    }

    #[tokio::test]
    async fn keeps_output_when_bash_command_times_out() {
        let input = json!({ "command": "echo before; echo problem >&2; sleep 5", "timeout": 1 });
        assert_eq!(
            call("bash", &input).await,
            Err("before\n\nstderr:\nproblem\n\ncommand timed out after 1s".into())
        );
    }

    #[tokio::test]
    async fn keeps_exit_status_when_output_is_truncated() {
        let input = json!({ "command": format!("head -c {} /dev/zero | tr '\\0' a; exit 3", MAX_OUTPUT * 2) });
        let text = call("bash", &input).await.unwrap();
        assert!(text.ends_with("exit status: 3"));
    }

    #[tokio::test]
    async fn keeps_start_and_end_of_long_output_from_both_streams() {
        let command = format!(
            "echo first; head -c {} /dev/zero | tr '\\0' a; echo last; echo problem >&2",
            MAX_OUTPUT * 50
        );
        let text = call("bash", &json!({ "command": command })).await.unwrap();
        assert!(text.starts_with("first\n"), "{}", &text[..20]);
        assert!(text.contains("bytes left out"));
        assert!(text.contains("last\n\nstderr:\nproblem\n"));
        assert!(text.len() <= MAX_OUTPUT + 100, "{}", text.len());
    }

    #[test]
    fn cuts_long_output_between_characters() {
        let mut captured = super::Captured::default();
        captured.push("é".repeat(MAX_OUTPUT * 2).as_bytes());
        captured.push(b"xy");
        let text = captured.text(MAX_OUTPUT - 1);
        assert!(!text.contains('\u{FFFD}'));
        assert!(text.starts_with('é') && text.ends_with("éxy"));
    }

    #[tokio::test]
    async fn gives_one_stream_the_room_the_other_does_not_use() {
        let command = format!("head -c {} /dev/zero | tr '\\0' a", MAX_OUTPUT * 3 / 4);
        let text = call("bash", &json!({ "command": command })).await.unwrap();
        assert_eq!(text.len(), MAX_OUTPUT * 3 / 4);
    }

    #[tokio::test]
    async fn reports_exit_status_once() {
        let input = json!({ "command": "echo hi; exit 2" });
        assert_eq!(
            call("bash", &input).await,
            Ok("hi\n\nexit status: 2".into())
        );
    }

    #[tokio::test]
    async fn returns_when_background_process_keeps_output_open() {
        let input = json!({ "command": "sleep 30 & echo $!", "timeout": 10 });
        let started = std::time::Instant::now();
        let text = call("bash", &input).await.unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        let killed = std::process::Command::new("kill")
            .arg(text.trim())
            .status()
            .unwrap()
            .success();
        assert!(killed);
    }

    #[tokio::test]
    async fn kills_background_processes_on_timeout() {
        let pid_file = TemporaryFile::new("pid", "");
        let command = format!("sleep 30 & echo $! > {}; wait", pid_file.path().display());
        let input = json!({ "command": command, "timeout": 1 });
        assert!(call("bash", &input).await.is_err());
        let pid = pid_file.content();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let alive = std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap()
            .success();
        assert!(!alive);
    }
}
