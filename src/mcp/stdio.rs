use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::ChildStdin,
    sync::mpsc,
};

use super::{
    DEFAULT_TIMEOUT, MAX_LINE, STDERR_TAIL,
    config_file::{ServerConfig, expand_home},
    connection::{Connection, Pending, handle_message},
};
use crate::tools::{self, ProcessGroup};

/// The server's process group, shared by the connection and the task that
/// reaps the server. Whichever lets go first kills the group; the other
/// then finds it gone, so a recycled group id is never signalled.
pub(super) struct ServerGroup(Arc<Mutex<ProcessGroup>>);

impl Drop for ServerGroup {
    fn drop(&mut self) {
        if let Ok(mut group) = self.0.lock() {
            drop(ProcessGroup(group.0.take()));
        }
    }
}

/// Writes each queued line in full, even if the request that queued it was
/// cancelled, so the server never sees a line cut off midway.
async fn write_messages(
    mut stdin: ChildStdin,
    mut messages: mpsc::UnboundedReceiver<Value>,
    pending: Arc<Pending>,
) {
    while let Some(message) = messages.recv().await {
        let line = format!("{message}\n");
        if stdin.write_all(line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
    pending.close();
}

async fn read_messages(
    stdout: impl tokio::io::AsyncRead + Unpin,
    writer: mpsc::UnboundedSender<Value>,
    pending: Arc<Pending>,
) {
    let mut stdout = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut stdout)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)
            .await
        {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if line.len() > MAX_LINE && line.last() != Some(&b'\n') {
            let error = format!(
                "server sent a line longer than {} MiB; closed the connection",
                MAX_LINE >> 20
            );
            if let Ok(mut senders) = pending.senders.lock() {
                for (_, sender) in senders.drain() {
                    let _ = sender.send(Err(error.clone()));
                }
            }
            break;
        }
        let Ok(message) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        handle_message(&message, &writer, &pending);
    }
    pending.close();
}

async fn read_stderr(mut stderr: impl tokio::io::AsyncRead + Unpin, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0; 4096];
    while let Ok(count) = stderr.read(&mut chunk).await
        && count > 0
    {
        if let Ok(mut tail) = tail.lock() {
            tail.extend_from_slice(&chunk[..count]);
            let mut excess = tail.len().saturating_sub(STDERR_TAIL);
            while tail.get(excess).is_some_and(|byte| byte & 0xC0 == 0x80) {
                excess += 1;
            }
            tail.drain(..excess);
        }
    }
}

impl Connection {
    pub(super) fn spawn(config: &ServerConfig, command: &str) -> Result<Self, String> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut process = tokio::process::Command::new(expand_home(command, home.as_deref()));
        process
            .args(
                config
                    .args
                    .iter()
                    .map(|argument| expand_home(argument, home.as_deref())),
            )
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &config.cwd {
            let cwd = expand_home(cwd, home.as_deref());
            if !Path::new(&cwd).is_dir() {
                return Err(format!("cwd {cwd} is not a folder"));
            }
            process.current_dir(cwd);
        }
        tools::new_session(&mut process);
        let mut child = process
            .spawn()
            .map_err(|error| format!("failed to start {command}: {error}"))?;
        let process_group = Arc::new(Mutex::new(ProcessGroup(child.id())));
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err("failed to capture server input and output".into());
        };
        let (writer, lines) = mpsc::unbounded_channel();
        let pending = Arc::new(Pending::default());
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(write_messages(stdin, lines, pending.clone()));
        tokio::spawn(read_messages(stdout, writer.clone(), pending.clone()));
        tokio::spawn(read_stderr(stderr, stderr_tail.clone()));
        // Reap the server if it exits on its own and stop anything it left
        // running; dropping the connection kills its process group first,
        // after which this wait returns too.
        let reaper_group = ServerGroup(process_group.clone());
        tokio::spawn(async move {
            let _ = child.wait().await;
            drop(reaper_group);
        });
        Ok(Self {
            writer,
            pending,
            next_id: AtomicU64::new(1),
            stderr: stderr_tail,
            timeout: Duration::from_secs(config.timeout.unwrap_or(DEFAULT_TIMEOUT)),
            _process_group: Some(ServerGroup(process_group)),
            session: None,
        })
    }
}
