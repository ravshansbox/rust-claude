use std::{
    process::{Output, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use tokio::process::Command;

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
pub async fn read_image() -> Result<Option<Vec<u8>>> {
    use std::sync::atomic::{AtomicU64, Ordering};
    // Each paste gets its own file, since several pastes may run at once.
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "rust-coding-agent-clipboard-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    // Remove the file however this returns, including a helper timing out
    // after writing part of it.
    struct RemoveOnDrop<'a>(&'a std::path::Path);
    impl Drop for RemoveOnDrop<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _remove = RemoveOnDrop(&path);
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
        )
        .await?
        .status;
        if status.success() {
            return Ok(Some(tokio::fs::read(&path).await?));
        }
    }
    Ok(None)
}

#[cfg(not(target_os = "macos"))]
pub async fn read_image() -> Result<Option<Vec<u8>>> {
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
    let types = output(Command::new(program).args(list_types), HELPER_TIMEOUT).await?;
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
    )
    .await?;
    Ok(Some(output.stdout).filter(|data| output.status.success() && !data.is_empty()))
}

/// Runs a clipboard helper and collects what it prints, killing it if it
/// takes longer than `timeout`.
async fn output(command: &mut Command, timeout: Duration) -> Result<Output> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;
    tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            anyhow!(
                "{program} did not answer within {} seconds",
                timeout.as_secs_f64()
            )
        })?
        .with_context(|| format!("failed to read from {program}"))
}

#[cfg(test)]
mod tests {
    use super::output;
    use std::time::{Duration, Instant};
    use tokio::process::Command;

    #[tokio::test]
    async fn returns_what_a_helper_prints() {
        let printed = output(
            Command::new("sh").args(["-c", "printf image"]),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(printed.status.success());
        assert_eq!(printed.stdout, b"image");
    }

    #[tokio::test]
    async fn stops_a_helper_that_does_not_answer() {
        let started = Instant::now();
        let error = output(Command::new("sleep").arg("30"), Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(error.to_string(), "sleep did not answer within 0.2 seconds");
    }
}
