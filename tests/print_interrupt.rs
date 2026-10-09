#![cfg(unix)]

use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn wait_for(condition: impl Fn() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn is_running(pid: &str) -> bool {
    Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn ctrl_c_in_print_mode_stops_started_processes() {
    let root = std::env::temp_dir().join(format!("rust-claude-interrupt-{}", std::process::id()));
    let home = root.join("home");
    let project = root.join("project");
    let pid_file = root.join("server.pid");
    write(
        &home.join(".rust-claude").join("auth.json"),
        r#"{ "access": "access", "refresh": "refresh", "expires": 4102444800000 }"#,
    );
    let server = serde_json::json!({
        "mcpServers": {
            "hang": {
                "command": "bash",
                "args": ["-c", format!("echo $$ > {}; exec sleep 30", pid_file.display())],
            }
        }
    });
    write(
        &project.join(".rust-claude").join("mcp.json"),
        &server.to_string(),
    );

    let child = Command::new(env!("CARGO_BIN_EXE_rust-claude"))
        .args(["-p", "hello"])
        .current_dir(&project)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started =
        wait_for(|| std::fs::read_to_string(&pid_file).is_ok_and(|pid| !pid.trim().is_empty()));
    let server_pid = std::fs::read_to_string(&pid_file).unwrap_or_default();
    let server_pid = server_pid.trim();
    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let server_stopped = wait_for(|| !is_running(server_pid));
    if !server_stopped {
        let _ = Command::new("kill").args(["-KILL", server_pid]).status();
    }
    let _ = std::fs::remove_dir_all(&root);

    assert!(started);
    assert_eq!(output.status.code(), Some(130));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cancelled"));
    assert!(server_stopped);
}
