use std::{path::Path, process::Stdio};

use anyhow::{Context, Result};

use super::{PACKAGE, Updater};

impl Updater {
    /// Runs cargo, and rustc for the build folder, from the update folder,
    /// so rustup ignores any `rust-toolchain.toml` of the project open here.
    pub(super) async fn build(&self, cargo: &Path, folder: &Path) -> Result<()> {
        let mut command = tokio::process::Command::new(cargo);
        command
            .kill_on_drop(true)
            .current_dir(folder)
            .args(["install", "--locked", "--force"]);
        if let Some(build_dir) = &self.build_dir {
            self.prepare_build_dir(build_dir, folder).await;
            command.arg("--target-dir").arg(build_dir);
        }
        let output = command
            .arg("--path")
            .arg(folder.join(PACKAGE))
            .stdin(Stdio::null())
            .output()
            .await
            .context("running cargo")?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let reason = stderr
            .lines()
            .find(|line| line.starts_with("error"))
            .map_or_else(
                || format!("cargo install exited with {}", output.status),
                str::to_string,
            );
        anyhow::bail!(reason)
    }

    async fn prepare_build_dir(&self, build_dir: &Path, folder: &Path) {
        let Some(rustc) = &self.rustc else {
            return;
        };
        let Ok(output) = tokio::process::Command::new(rustc)
            .current_dir(folder)
            .arg("-vV")
            .stdin(Stdio::null())
            .output()
            .await
        else {
            return;
        };
        if !output.status.success() {
            return;
        }
        let stamp = build_dir.join("rustc-version");
        if std::fs::read(&stamp).ok().as_deref() == Some(output.stdout.as_slice()) {
            return;
        }
        let _ = std::fs::remove_dir_all(build_dir);
        let _ = std::fs::create_dir_all(build_dir);
        let _ = std::fs::write(stamp, output.stdout);
    }
}
