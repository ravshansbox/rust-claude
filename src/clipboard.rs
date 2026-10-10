use std::{
    io::Read,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

/// How long a clipboard helper may take. One waiting on a clipboard owner
/// that never answers would otherwise run forever.
const HELPER_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(not(target_os = "macos"))]
const IMAGE_TYPES: [&str; 5] = [
    "image/png",
    "image/jpeg",
    "image/webp",
    "image/gif",
    "image/tiff",
];

#[cfg(target_os = "macos")]
pub fn read_image() -> Result<Option<Vec<u8>>> {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Each paste gets its own file, since several pastes may run at once.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "rust-claude-clipboard-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    for class in ["PNGf", "TIFF"] {
        let status = output(
            Command::new("osascript")
                .args([
                    "-e",
                    "on run argv",
                    "-e",
                    &format!("set imageData to the clipboard as «class {class}»"),
                    "-e",
                    "set fileReference to open for access POSIX file (item 1 of argv) with write permission",
                    "-e",
                    "set eof fileReference to 0",
                    "-e",
                    "write imageData to fileReference",
                    "-e",
                    "close access fileReference",
                    "-e",
                    "end run",
                ])
                .arg(&path),
            HELPER_TIMEOUT,
        )?
        .status;
        if status.success() {
            let data = std::fs::read(&path);
            let _ = std::fs::remove_file(&path);
            return Ok(Some(data?));
        }
    }
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
pub fn read_image() -> Result<Option<Vec<u8>>> {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (program, list_types, read_type): (&str, &[&str], &[&str]) = if wayland {
        ("wl-paste", &["--list-types"], &["--no-newline", "--type"])
    } else {
        (
            "xclip",
            &["-selection", "clipboard", "-t", "TARGETS", "-o"],
            &["-selection", "clipboard", "-o", "-t"],
        )
    };
    let types = output(Command::new(program).args(list_types), HELPER_TIMEOUT)?;
    let types = String::from_utf8_lossy(&types.stdout);
    let Some(image_type) = IMAGE_TYPES
        .iter()
        .find(|image_type| types.lines().any(|line| line.trim() == **image_type))
    else {
        return Ok(None);
    };
    let output = output(
        Command::new(program).args(read_type).arg(image_type),
        HELPER_TIMEOUT,
    )?;
    Ok(Some(output.stdout).filter(|data| output.status.success() && !data.is_empty()))
}

/// Runs a clipboard helper and collects what it prints, killing it if it
/// takes longer than `timeout`.
fn output(command: &mut Command, timeout: Duration) -> Result<Output> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;
    // Read while waiting, so a large image cannot fill the pipe and stall it.
    let mut stdout = child.stdout.take().context("no output pipe")?;
    let reader = std::thread::spawn(move || {
        let mut data = Vec::new();
        stdout.read_to_end(&mut data).map(|_| data)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "{program} did not answer within {} seconds",
                timeout.as_secs_f64()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = reader
        .join()
        .map_err(|_| anyhow::anyhow!("failed to read from {program}"))?
        .with_context(|| format!("failed to read from {program}"))?;
    Ok(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::output;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    #[test]
    fn returns_what_a_helper_prints() {
        let printed = output(
            Command::new("sh").args(["-c", "printf image"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(printed.status.success());
        assert_eq!(printed.stdout, b"image");
    }

    #[test]
    fn stops_a_helper_that_does_not_answer() {
        let started = Instant::now();
        let error =
            output(Command::new("sleep").arg("30"), Duration::from_millis(200)).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(error.to_string(), "sleep did not answer within 0.2 seconds");
    }
}
