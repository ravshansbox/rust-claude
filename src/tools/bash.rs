use super::{MAX_OUTPUT, argument, truncate};
use serde_json::{Value, json};
use std::{
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncReadExt;

pub(super) fn definition() -> Value {
    json!({
        "name": "bash",
        "description": "Run a bash command in the current project and return stdout and stderr",
        "input_schema": {
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeout": { "type": "integer", "description": "Seconds before the command is killed. No limit when omitted" }
            },
            "required": ["command"]
        }
    })
}

pub(super) async fn run(input: &Value) -> Result<String, String> {
    let timeout = match &input["timeout"] {
        Value::Null => None,
        value => match value.as_u64() {
            Some(seconds) if seconds > 0 => Some(seconds),
            _ => return Err("timeout must be at least 1".into()),
        },
    };
    let mut command = tokio::process::Command::new("bash");
    command
        .args(["-c", argument(input, "command")?])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run command: {error}"))?;
    let mut process_group = ProcessGroup(child.id());
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err("failed to capture command output".into());
    };
    let stdout_output = Arc::new(Mutex::new(Vec::new()));
    let stderr_output = Arc::new(Mutex::new(Vec::new()));
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

async fn read_capped(mut reader: impl tokio::io::AsyncRead + Unpin, output: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0; 8192];
    while let Ok(count) = reader.read(&mut chunk).await
        && count > 0
    {
        if let Ok(mut output) = output.lock() {
            let room = (MAX_OUTPUT + 1).saturating_sub(output.len());
            output.extend_from_slice(&chunk[..count.min(room)]);
        }
    }
}

fn snapshot(output: &Mutex<Vec<u8>>) -> Vec<u8> {
    output
        .lock()
        .map(|output| output.clone())
        .unwrap_or_default()
}

fn output_text(stdout_output: &Mutex<Vec<u8>>, stderr_output: &Mutex<Vec<u8>>) -> String {
    let mut text = String::from_utf8_lossy(&snapshot(stdout_output)).into_owned();
    let stderr = String::from_utf8_lossy(&snapshot(stderr_output)).into_owned();
    if !stderr.is_empty() {
        text.push_str("\nstderr:\n");
        text.push_str(&stderr);
    }
    truncate(text)
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
    async fn keeps_only_start_of_long_output() {
        let input = json!({ "command": format!("head -c {} /dev/zero | tr '\\0' a; echo b >&2", MAX_OUTPUT * 50) });
        let text = call("bash", &input).await.unwrap();
        assert!(text.ends_with("… output truncated"));
        assert!(!text.contains("stderr"));
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
