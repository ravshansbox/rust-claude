use std::{
    ffi::OsStr,
    io::{self, Cursor},
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::{config, settings::Settings};

mod progress;

pub use progress::Progress;

pub const RELEASES_URL: &str =
    "https://api.github.com/repos/ravshansbox/rust-coding-agent/releases/latest";
const PACKAGE: &str = "rust-coding-agent";
const ASSET: &str = "rust-coding-agent.zip";
const SIGNATURE_ASSET: &str = "rust-coding-agent.zip.minisig";
const PUBLIC_KEY: &str = "RWSa19cgsH6sJ8TomZ0rN68H2QSZmUNhCpxJtnW6IQW/gn6XDhilg5A8";
static RUNS: AtomicUsize = AtomicUsize::new(0);

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub struct Updater {
    pub http: reqwest::Client,
    pub releases_url: String,
    pub current_version: &'static str,
    pub cargo: Option<PathBuf>,
    pub rustc: Option<PathBuf>,
    pub build_dir: Option<PathBuf>,
    pub bin_dir: Option<PathBuf>,
    pub update_dir: PathBuf,
    pub public_key: &'static str,
}

impl Updater {
    /// The updater for this build, unless it is a debug build, as from
    /// `cargo run`, or `check_for_updates` is false in settings.json.
    pub fn for_this_build(settings: &Settings) -> Option<Self> {
        Self::new(settings, cfg!(debug_assertions))
    }

    fn new(settings: &Settings, debug_build: bool) -> Option<Self> {
        if debug_build || settings.check_for_updates == Some(false) {
            return None;
        }
        let config_dir = config::dir()?;
        let path = std::env::var_os("PATH");
        Some(Self {
            http: reqwest::Client::new(),
            releases_url: RELEASES_URL.into(),
            current_version: env!("CARGO_PKG_VERSION"),
            cargo: find_on_path("cargo", path.as_deref()),
            rustc: find_on_path("rustc", path.as_deref()),
            build_dir: Some(config_dir.join("build")),
            bin_dir: cargo_bin_dir(),
            update_dir: config_dir.join("updates"),
            public_key: PUBLIC_KEY,
        })
    }

    /// Checks for a newer release on every launch, and builds and
    /// installs it with cargo. Builds from `main`, at version 0.0.0, never
    /// check. A failed check stays quiet, as when offline.
    pub async fn run(self, report: impl Fn(Progress)) {
        let Some(current) = parse_version(self.current_version) else {
            return;
        };
        if current == [0, 0, 0] {
            return;
        }
        let Ok(release) = self.latest_release().await else {
            return;
        };
        if parse_version(&release.tag_name).is_none_or(|latest| latest <= current) {
            return;
        }
        let version = format!("v{}", release.tag_name.trim_start_matches('v'));
        report(Progress::Available(version.clone()));
        let Some(cargo) = &self.cargo else {
            report(Progress::CargoMissing);
            return;
        };
        let folder = self.update_dir.join(format!(
            "{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(error) = config::create_private_dir(&self.update_dir)
            .and_then(|()| create_update_folder(&folder))
        {
            report(Progress::Failed(format!(
                "creating {}: {error}",
                folder.display()
            )));
            return;
        }
        let folder = UpdateFolder(folder);
        let outcome = self
            .install(&release, &version, cargo, &folder.0, &report)
            .await;
        drop(folder);
        match outcome {
            Ok(()) => {
                if let Some(bin_dir) = &self.bin_dir {
                    link_alias_to(bin_dir, OsStr::new(PACKAGE));
                }
                report(Progress::Ready);
            }
            Err(failure) => report(failure),
        }
    }

    async fn install(
        &self,
        release: &Release,
        version: &str,
        cargo: &Path,
        folder: &Path,
        report: &impl Fn(Progress),
    ) -> Result<(), Progress> {
        let asset_url = |name: &str| {
            release
                .assets
                .iter()
                .find(|asset| asset.name == name)
                .map(|asset| asset.browser_download_url.clone())
                .with_context(|| format!("the release has no {name}"))
                .map_err(|error| Progress::Failed(format!("{error:#}")))
        };
        let url = asset_url(ASSET)?;
        let signature_url = asset_url(SIGNATURE_ASSET)?;
        report(Progress::Downloading);
        let started = Instant::now();
        self.download(&url, &signature_url, version, folder)
            .await
            .map_err(|error| Progress::DownloadFailed(format!("{error:#}"), started.elapsed()))?;
        report(Progress::Downloaded(started.elapsed()));
        report(Progress::Building);
        let started = Instant::now();
        self.build(cargo, folder)
            .await
            .map_err(|error| Progress::BuildFailed(format!("{error:#}"), started.elapsed()))?;
        report(Progress::Built(started.elapsed()));
        Ok(())
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header(reqwest::header::USER_AGENT, PACKAGE)
    }

    async fn latest_release(&self) -> Result<Release> {
        Ok(self
            .get(&self.releases_url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        Ok(self
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?
            .into())
    }

    /// Downloads the zip and unpacks it, once its signature shows it was
    /// made with the release key.
    async fn download(
        &self,
        url: &str,
        signature_url: &str,
        version: &str,
        folder: &Path,
    ) -> Result<()> {
        let bytes = self.fetch(url).await?;
        let signature = self.fetch(signature_url).await?;
        let signed_as = minisign_verify::PublicKey::from_base64(self.public_key)
            .and_then(|key| {
                let signature =
                    minisign_verify::Signature::decode(&String::from_utf8_lossy(&signature))?;
                key.verify(&bytes, &signature, false)?;
                Ok(signature.trusted_comment().to_string())
            })
            .map_err(|_| anyhow::anyhow!("the signature of {ASSET} is not valid"))?;
        if signed_as.strip_prefix("rust-coding-agent ") != Some(version) {
            anyhow::bail!("{ASSET} is signed as {signed_as}, not {version}");
        }
        let target = folder.to_path_buf();
        tokio::task::spawn_blocking(move || {
            zip::ZipArchive::new(Cursor::new(bytes))?.extract(target)
        })
        .await?
        .with_context(|| format!("unpacking {ASSET}"))
    }

    /// Runs cargo, and rustc for the build folder, from the update folder,
    /// so rustup ignores any `rust-toolchain.toml` of the project open here.
    async fn build(&self, cargo: &Path, folder: &Path) -> Result<()> {
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

fn create_update_folder(folder: &Path) -> io::Result<()> {
    config::private_dir().create(folder)
}

/// An update's own folder, deleted when the update ends, even when the app
/// quits halfway and drops it.
struct UpdateFolder(PathBuf);

impl Drop for UpdateFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn link_alias_to(bin_dir: &Path, program: &OsStr) {
    let alias = bin_dir.join("rca");
    if alias.symlink_metadata().is_err() {
        let _ = std::fs::create_dir_all(bin_dir);
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(program, &alias);
    }
}

/// Adds the `rca` link beside this program if nothing has that name, so
/// every way of installing gets it. Debug builds, as from `cargo run`, skip it.
pub fn link_alias_beside_this_program() {
    if let Ok(program) = std::env::current_exe() {
        link_alias_beside(&program, cfg!(debug_assertions));
    }
}

fn link_alias_beside(program: &Path, debug_build: bool) {
    if debug_build {
        return;
    }
    if let (Some(dir), Some(name)) = (program.parent(), program.file_name()) {
        link_alias_to(dir, name);
    }
}

fn cargo_bin_dir() -> Option<PathBuf> {
    let root = std::env::var_os("CARGO_INSTALL_ROOT")
        .or_else(|| std::env::var_os("CARGO_HOME"))
        .map(PathBuf::from)
        .or_else(|| config::home().map(|home| home.join(".cargo")))?;
    Some(root.join("bin"))
}

fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.trim_start_matches('v').split('.').map(str::parse);
    let version = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    parts.next().is_none().then_some(version)
}

pub fn find_on_path(program: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let file = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path?)
        .map(|directory| directory.join(&file))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests;
