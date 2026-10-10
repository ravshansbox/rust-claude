use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const RELEASES_URL: &str =
    "https://api.github.com/repos/ravshansbox/rust-claude/releases/latest";
const ASSET: &str = "rust-claude.zip";
static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Available(String),
    CargoMissing(String),
    Downloading(String),
    Building(String),
    Ready(String, std::time::Duration),
    Failed(String, String, Option<std::time::Duration>),
}

impl Progress {
    pub fn message(&self) -> String {
        match self {
            Self::Available(version) => format!("update {version} available"),
            Self::CargoMissing(version) => {
                format!("cargo not found on PATH, install Rust to update to {version}")
            }
            Self::Downloading(version) => format!("downloading {version}"),
            Self::Building(version) => format!("building {version}"),
            Self::Ready(version, took) => format!(
                "installed {version} in {}, restart rust-claude to use it",
                format_took(*took)
            ),
            Self::Failed(version, error, None) => format!("update to {version} failed: {error}"),
            Self::Failed(version, error, Some(took)) => format!(
                "update to {version} failed after {}: {error}",
                format_took(*took)
            ),
        }
    }
}

fn format_took(took: std::time::Duration) -> String {
    let seconds = took.as_secs();
    match seconds / 60 {
        0 => format!("{seconds}s"),
        minutes => format!("{minutes}m {}s", seconds % 60),
    }
}

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
}

impl Updater {
    /// The updater for this build, unless it is a debug build, as from
    /// `cargo run`, or `check_for_updates` is false in settings.json.
    pub fn for_this_build(settings: &crate::settings::Settings) -> Option<Self> {
        Self::new(settings, cfg!(debug_assertions))
    }

    fn new(settings: &crate::settings::Settings, debug_build: bool) -> Option<Self> {
        if debug_build || settings.check_for_updates == Some(false) {
            return None;
        }
        Some(Self {
            http: reqwest::Client::new(),
            releases_url: RELEASES_URL.into(),
            current_version: env!("CARGO_PKG_VERSION"),
            cargo: find_on_path("cargo", std::env::var_os("PATH")),
            rustc: find_on_path("rustc", std::env::var_os("PATH")),
            build_dir: crate::config::dir().map(|dir| dir.join("build")),
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
            report(Progress::CargoMissing(version));
            return;
        };
        let folder = std::env::temp_dir().join(format!(
            "rust-claude-update-{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let outcome = async {
            self.download(&release, &version, &folder, &report)
                .await
                .map_err(|error| (error, None))?;
            report(Progress::Building(version.clone()));
            let started = std::time::Instant::now();
            self.build(cargo, &folder)
                .await
                .map_err(|error| (error, Some(started.elapsed())))?;
            Ok(started.elapsed())
        }
        .await;
        let _ = std::fs::remove_dir_all(&folder);
        match outcome {
            Ok(took) => report(Progress::Ready(version, took)),
            Err((error, took)) => report(Progress::Failed(version, format!("{error:#}"), took)),
        }
    }

    async fn latest_release(&self) -> Result<Release> {
        Ok(self
            .http
            .get(&self.releases_url)
            .header(reqwest::header::USER_AGENT, "rust-claude")
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn download(
        &self,
        release: &Release,
        version: &str,
        folder: &Path,
        report: &impl Fn(Progress),
    ) -> Result<()> {
        let url = release
            .assets
            .iter()
            .find(|asset| asset.name == ASSET)
            .map(|asset| asset.browser_download_url.clone())
            .with_context(|| format!("the release has no {ASSET}"))?;
        report(Progress::Downloading(version.into()));
        let bytes = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, "rust-claude")
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let target = folder.to_path_buf();
        tokio::task::spawn_blocking(move || {
            zip::ZipArchive::new(std::io::Cursor::new(bytes))?.extract(target)
        })
        .await?
        .with_context(|| format!("unpacking {ASSET}"))?;
        Ok(())
    }

    async fn build(&self, cargo: &Path, folder: &Path) -> Result<()> {
        let mut command = tokio::process::Command::new(cargo);
        command.args(["install", "--locked", "--force"]);
        if let Some(build_dir) = &self.build_dir {
            self.prepare_build_dir(build_dir).await;
            command.arg("--target-dir").arg(build_dir);
        }
        let output = command
            .arg("--path")
            .arg(folder.join("rust-claude"))
            .stdin(std::process::Stdio::null())
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
            .map(str::to_string)
            .unwrap_or_else(|| format!("cargo install exited with {}", output.status));
        anyhow::bail!(reason)
    }

    async fn prepare_build_dir(&self, build_dir: &Path) {
        let Some(rustc) = &self.rustc else {
            return;
        };
        let Ok(output) = tokio::process::Command::new(rustc)
            .arg("-vV")
            .stdin(std::process::Stdio::null())
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

fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.trim_start_matches('v').split('.').map(str::parse);
    let version = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    parts.next().is_none().then_some(version)
}

pub fn find_on_path(program: &str, path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let file = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&path?)
        .map(|directory| directory.join(&file))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::{Progress, Updater, find_on_path};
    use std::{
        io::Write,
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("rust-claude-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn source_zip() -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("rust-claude/Cargo.toml", options).unwrap();
        zip.write_all(b"[package]\nname = \"rust-claude\"\nversion = \"0.2.0\"\n")
            .unwrap();
        zip.finish().unwrap();
        buffer.into_inner()
    }

    /// Serves the latest release on `/latest` and its zip on
    /// `/rust-claude.zip`, and counts the requests.
    async fn github(tag: &str) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let release = serde_json::json!({
            "tag_name": tag,
            "assets": [{ "name": "rust-claude.zip", "browser_download_url": format!("{base}/rust-claude.zip") }],
        })
        .to_string()
        .into_bytes();
        let zip = source_zip();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut data = Vec::new();
                let mut chunk = [0u8; 4096];
                while !data.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => data.extend_from_slice(&chunk[..read]),
                    }
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let path = request.split_whitespace().nth(1).unwrap_or("").to_string();
                recorded.lock().unwrap().push(request);
                let body = if path == "/rust-claude.zip" {
                    &zip
                } else {
                    &release
                };
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(body).await;
            }
        });
        (base, requests)
    }

    /// A stand-in for cargo that records its arguments and whether the
    /// folder after `--path` holds the unpacked Cargo.toml.
    fn fake_cargo(dir: &Path, exit_code: i32) -> PathBuf {
        let cargo = dir.join("cargo");
        let log = dir.join("cargo.log");
        std::fs::write(
            &cargo,
            format!(
                "#!/bin/sh\necho \"$@\" > '{log}'\nfor last; do :; done\ngrep -q 'version = \"0.2.0\"' \"$last/Cargo.toml\" && echo unpacked >> '{log}'\necho 'error: could not compile' >&2\nexit {exit_code}\n",
                log = log.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
        cargo
    }

    fn fake_rustc(dir: &Path, version: &str) -> PathBuf {
        let rustc = dir.join("rustc");
        std::fs::write(&rustc, format!("#!/bin/sh\necho '{version}'\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).unwrap();
        rustc
    }

    async fn run(updater: Updater) -> Vec<String> {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let recorded = shown.clone();
        updater
            .run(move |progress: Progress| recorded.lock().unwrap().push(progress.message()))
            .await;
        Arc::try_unwrap(shown).unwrap().into_inner().unwrap()
    }

    fn updater(base: &str, cargo: Option<PathBuf>) -> Updater {
        Updater {
            http: reqwest::Client::new(),
            releases_url: format!("{base}/latest"),
            current_version: "0.1.0",
            cargo,
            rustc: None,
            build_dir: None,
        }
    }

    #[tokio::test]
    async fn downloads_and_builds_a_newer_release_and_offers_a_restart() {
        let dir = temp_dir("newer");
        let (base, _) = github("v0.2.0").await;
        let cargo = fake_cargo(&dir, 0);
        let shown = run(updater(&base, Some(cargo))).await;
        assert_eq!(
            shown[..3],
            [
                "update v0.2.0 available",
                "downloading v0.2.0",
                "building v0.2.0",
            ]
        );
        assert_eq!(shown.len(), 4, "{shown:?}");
        assert!(
            shown[3].starts_with("installed v0.2.0 in ")
                && shown[3].ends_with("s, restart rust-claude to use it"),
            "{shown:?}"
        );
        let log = std::fs::read_to_string(dir.join("cargo.log")).unwrap();
        assert!(log.starts_with("install --locked --force --path "), "{log}");
        assert!(log.contains("unpacked"), "{log}");
    }

    #[tokio::test]
    async fn builds_in_the_build_folder() {
        let dir = temp_dir("build-folder");
        let (base, _) = github("v0.2.0").await;
        let mut updater = updater(&base, Some(fake_cargo(&dir, 0)));
        updater.build_dir = Some(dir.join("build"));
        run(updater).await;
        let log = std::fs::read_to_string(dir.join("cargo.log")).unwrap();
        let expected = format!(
            "install --locked --force --target-dir {} --path ",
            dir.join("build").display()
        );
        assert!(log.starts_with(&expected), "{log}");
    }

    #[tokio::test]
    async fn keeps_the_build_folder_while_the_compiler_stays_the_same() {
        let dir = temp_dir("same-compiler");
        let (base, _) = github("v0.2.0").await;
        let build = dir.join("build");
        for _ in 0..2 {
            let mut updater = updater(&base, Some(fake_cargo(&dir, 0)));
            updater.rustc = Some(fake_rustc(&dir, "rustc 1.99.0"));
            updater.build_dir = Some(build.clone());
            run(updater).await;
            std::fs::write(build.join("earlier-build"), "").unwrap();
        }
        assert!(build.join("earlier-build").exists());
    }

    #[tokio::test]
    async fn empties_the_build_folder_when_the_compiler_changes() {
        let dir = temp_dir("new-compiler");
        let (base, _) = github("v0.2.0").await;
        let build = dir.join("build");
        for version in ["rustc 1.98.0", "rustc 1.99.0"] {
            let mut updater = updater(&base, Some(fake_cargo(&dir, 0)));
            updater.rustc = Some(fake_rustc(&dir, version));
            updater.build_dir = Some(build.clone());
            run(updater).await;
            std::fs::write(build.join(version), "").unwrap();
        }
        assert!(!build.join("rustc 1.98.0").exists());
        assert!(build.join("rustc 1.99.0").exists());
    }

    #[tokio::test]
    async fn shows_why_building_failed() {
        let dir = temp_dir("failed");
        let (base, _) = github("v0.2.0").await;
        let cargo = fake_cargo(&dir, 101);
        let shown = run(updater(&base, Some(cargo))).await;
        let last = shown.last().unwrap();
        assert!(
            last.starts_with("update to v0.2.0 failed after ")
                && last.ends_with("s: error: could not compile"),
            "{shown:?}"
        );
    }

    #[test]
    fn shows_how_long_building_took() {
        let ready = |seconds| {
            Progress::Ready("v0.2.0".into(), std::time::Duration::from_secs(seconds)).message()
        };
        assert_eq!(
            ready(0),
            "installed v0.2.0 in 0s, restart rust-claude to use it"
        );
        assert_eq!(
            ready(42),
            "installed v0.2.0 in 42s, restart rust-claude to use it"
        );
        assert_eq!(
            ready(125),
            "installed v0.2.0 in 2m 5s, restart rust-claude to use it"
        );
        assert_eq!(
            Progress::Failed(
                "v0.2.0".into(),
                "error: x".into(),
                Some(std::time::Duration::from_secs(70))
            )
            .message(),
            "update to v0.2.0 failed after 1m 10s: error: x"
        );
        assert_eq!(
            Progress::Failed("v0.2.0".into(), "no zip".into(), None).message(),
            "update to v0.2.0 failed: no zip"
        );
    }

    #[tokio::test]
    async fn warns_when_cargo_is_missing() {
        let (base, requests) = github("v0.2.0").await;
        let shown = run(updater(&base, None)).await;
        assert_eq!(
            shown,
            [
                "update v0.2.0 available",
                "cargo not found on PATH, install Rust to update to v0.2.0",
            ]
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stays_quiet_when_the_latest_release_is_not_newer() {
        for tag in ["v0.1.0", "v0.0.9"] {
            let (base, _) = github(tag).await;
            let shown = run(updater(&base, None)).await;
            assert!(shown.is_empty(), "{tag}: {shown:?}");
        }
    }

    #[tokio::test]
    async fn compares_versions_by_number() {
        let (base, _) = github("v0.10.0").await;
        let mut updater = updater(&base, None);
        updater.current_version = "0.9.0";
        assert_eq!(run(updater).await[0], "update v0.10.0 available");
    }

    #[tokio::test]
    async fn checks_on_every_launch() {
        let (base, requests) = github("v0.1.0").await;
        run(updater(&base, None)).await;
        run(updater(&base, None)).await;
        assert_eq!(requests.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn skips_builds_from_main() {
        let (base, requests) = github("v0.2.0").await;
        let mut updater = updater(&base, None);
        updater.current_version = "0.0.0";
        assert!(run(updater).await.is_empty());
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn can_be_turned_off_in_settings() {
        let settings = |check_for_updates| crate::settings::Settings {
            check_for_updates,
            ..Default::default()
        };
        assert!(Updater::new(&settings(Some(false)), false).is_none());
        assert!(Updater::new(&settings(Some(true)), false).is_some());
        assert!(Updater::new(&settings(None), false).is_some());
    }

    #[test]
    fn skips_debug_builds_such_as_cargo_run() {
        let settings = crate::settings::Settings {
            check_for_updates: Some(true),
            ..Default::default()
        };
        assert!(Updater::new(&settings, true).is_none());
        assert!(Updater::for_this_build(&settings).is_none());
    }

    #[test]
    fn finds_programs_on_path() {
        let dir = temp_dir("path");
        let cargo = fake_cargo(&dir, 0);
        let path = std::env::join_paths([dir.join("missing"), dir.clone()]).unwrap();
        assert_eq!(find_on_path("cargo", Some(path.clone())), Some(cargo));
        assert_eq!(find_on_path("rustc", Some(path)), None);
        assert_eq!(find_on_path("cargo", None), None);
    }
}
