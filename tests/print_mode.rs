#![cfg(unix)]

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
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

struct Setup {
    root: PathBuf,
    home: PathBuf,
    project: PathBuf,
    pid_file: PathBuf,
}

impl Setup {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("rust-claude-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Self {
            home: root.join("home"),
            project: root.join("project"),
            pid_file: root.join("server.pid"),
            root,
        }
    }

    fn write_sign_in(&self, config_dir: &Path) {
        write(
            &config_dir.join("auth.json"),
            r#"{ "access": "access", "refresh": "refresh", "expires": 4102444800000 }"#,
        );
    }

    fn write_hanging_server(&self, config_dir: &Path) {
        let server = serde_json::json!({
            "mcpServers": {
                "hang": {
                    "command": "bash",
                    "args": ["-c", format!("echo $$ > {}; exec sleep 30", self.pid_file.display())],
                }
            }
        });
        write(&config_dir.join("mcp.json"), &server.to_string());
    }

    fn run(&self, arguments: &[&str]) -> Child {
        Command::new(env!("CARGO_BIN_EXE_rust-claude"))
            .args(arguments)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn server_started(&self) -> bool {
        wait_for(|| std::fs::read_to_string(&self.pid_file).is_ok_and(|pid| !pid.trim().is_empty()))
    }

    fn server_pid(&self) -> String {
        std::fs::read_to_string(&self.pid_file)
            .unwrap_or_default()
            .trim()
            .to_string()
    }
}

fn interrupt(child: Child) -> std::process::Output {
    signal(child, "-INT")
}

/// Sends `signal` and waits up to the usual 10 s for the binary to exit,
/// killing it and failing the test if it does not.
fn signal(child: Child, signal: &str) -> std::process::Output {
    Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    let child = std::cell::RefCell::new(child);
    let exited = wait_for(|| child.borrow_mut().try_wait().unwrap().is_some());
    let mut child = child.into_inner();
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        panic!("rust-claude did not exit after {signal}");
    }
    child.wait_with_output().unwrap()
}

fn stop_server(pid: &str) -> bool {
    let stopped = wait_for(|| !is_running(pid));
    if !stopped && !pid.is_empty() {
        let _ = Command::new("kill").args(["-KILL", pid]).status();
    }
    stopped
}

#[test]
fn ctrl_c_in_print_mode_stops_started_processes() {
    let setup = Setup::new("interrupt");
    setup.write_sign_in(&setup.home.join(".rust-claude"));
    setup.write_hanging_server(&setup.project.join(".rust-claude"));

    let child = setup.run(&["-p", "hello"]);
    let started = setup.server_started();
    let output = interrupt(child);
    let server_stopped = stop_server(&setup.server_pid());
    let sessions: Vec<String> = std::fs::read_dir(setup.home.join(".rust-claude/sessions"))
        .into_iter()
        .flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
        .collect();
    let _ = std::fs::remove_dir_all(&setup.root);

    assert!(started);
    assert_eq!(output.status.code(), Some(130));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cancelled"));
    assert!(server_stopped);
    assert!(
        sessions
            .iter()
            .any(|session| session.contains(r#""content":"hello""#)
                && session.contains(r#""stop_reason":"aborted""#)),
        "prompt not saved as cancelled: {sessions:?}"
    );
}

#[test]
fn hangup_and_terminate_in_print_mode_stop_started_processes() {
    for (name, code) in [("HUP", 129), ("TERM", 143)] {
        let setup = Setup::new(&format!("signal-{name}"));
        setup.write_sign_in(&setup.home.join(".rust-claude"));
        setup.write_hanging_server(&setup.project.join(".rust-claude"));

        let child = setup.run(&["-p", "hello"]);
        let started = setup.server_started();
        let output = signal(child, &format!("-{name}"));
        let server_stopped = stop_server(&setup.server_pid());
        let _ = std::fs::remove_dir_all(&setup.root);

        assert!(started, "{name}");
        assert_eq!(output.status.code(), Some(code), "{name}");
        assert!(server_stopped, "{name}");
    }
}

#[test]
fn reads_sign_in_and_mcp_servers_from_config_dir() {
    let setup = Setup::new("config-dir");
    let config_dir = setup.root.join("config");
    setup.write_sign_in(&config_dir);
    setup.write_hanging_server(&config_dir);
    std::fs::create_dir_all(&setup.project).unwrap();
    std::fs::create_dir_all(&setup.home).unwrap();

    let child = setup.run(&["--config-dir", config_dir.to_str().unwrap(), "-p", "hello"]);
    let started = setup.server_started();
    let output = interrupt(child);
    stop_server(&setup.server_pid());
    let home_untouched = !setup.home.join(".rust-claude").exists();
    let _ = std::fs::remove_dir_all(&setup.root);

    assert!(
        started,
        "server did not start: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(home_untouched);
}

#[test]
fn reports_a_usage_error_for_an_argument_that_is_not_valid_text() {
    use std::os::unix::ffi::OsStrExt;

    let setup = Setup::new("non-utf8");
    std::fs::create_dir_all(&setup.project).unwrap();
    std::fs::create_dir_all(&setup.home).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rust-claude"))
        .args(["-p", "hello", "--image"])
        .arg(std::ffi::OsStr::from_bytes(b"photo-\xff.png"))
        .current_dir(&setup.project)
        .env("HOME", &setup.home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&setup.root);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("usage:"), "{stderr}");
}
