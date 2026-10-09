use std::process::Command;

use anyhow::{Context, Result};

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
        let status = Command::new("osascript")
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
            .arg(&path)
            .output()
            .context("failed to run osascript")?
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
    let types = Command::new(program)
        .args(list_types)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    let types = String::from_utf8_lossy(&types.stdout);
    let Some(image_type) = IMAGE_TYPES
        .iter()
        .find(|image_type| types.lines().any(|line| line.trim() == **image_type))
    else {
        return Ok(None);
    };
    let output = Command::new(program)
        .args(read_type)
        .arg(image_type)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    Ok(Some(output.stdout).filter(|data| output.status.success() && !data.is_empty()))
}
