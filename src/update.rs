use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};

use anyhow::{Context, Result};

use crate::{config, settings::Settings};

mod alias;
mod build;
mod download;
mod folder;
mod progress;

pub use alias::link_alias_beside_this_program;
use alias::link_alias_to;
use download::Release;
use folder::{UpdateFolder, create_update_folder};
pub use progress::Progress;

pub const RELEASES_URL: &str =
    "https://api.github.com/repos/ravshansbox/rust-coding-agent/releases/latest";
const PACKAGE: &str = "rust-coding-agent";
const ASSET: &str = "rust-coding-agent.zip";
const SIGNATURE_ASSET: &str = "rust-coding-agent.zip.minisig";
const PUBLIC_KEY: &str = "RWSa19cgsH6sJ8TomZ0rN68H2QSZmUNhCpxJtnW6IQW/gn6XDhilg5A8";
static RUNS: AtomicUsize = AtomicUsize::new(0);

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
