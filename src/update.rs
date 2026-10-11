use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const RELEASES_URL: &str =
    "https://api.github.com/repos/ravshansbox/rust-coding-agent/releases/latest";
const ASSET: &str = "rust-coding-agent.zip";
const SIGNATURE_ASSET: &str = "rust-coding-agent.zip.minisig";
const PUBLIC_KEY: &str = "RWSa19cgsH6sJ8TomZ0rN68H2QSZmUNhCpxJtnW6IQW/gn6XDhilg5A8";
static RUNS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Available(String),
    CargoMissing(String),
    Downloading(String),
    Downloaded(String, Duration),
    Building(String),
    Built(String, Duration),
    Ready(String),
    DownloadFailed(String, String, Duration),
    BuildFailed(String, String, Duration),
    Failed(String, String),
}

impl Progress {
    pub fn message(&self) -> String {
        match self {
            Self::Available(version) => format!("update {version} available"),
            Self::CargoMissing(_) => "cargo not found on PATH, install Rust to update".into(),
            Self::Downloading(_) => "downloading".into(),
            Self::Downloaded(_, took) => format!("downloaded in {}", format_took(*took)),
            Self::Building(_) => "building".into(),
            Self::Built(_, took) => format!("built in {}", format_took(*took)),
            Self::Ready(_) => "installed, restart rust-coding-agent to use it".into(),
            Self::DownloadFailed(_, error, took) => {
                format!("download failed after {}: {error}", format_took(*took))
            }
            Self::BuildFailed(_, error, took) => {
                format!("build failed after {}: {error}", format_took(*took))
            }
            Self::Failed(_, error) => format!("failed: {error}"),
        }
    }

    pub fn finishes_step(&self) -> bool {
        matches!(
            self,
            Self::Downloaded(..)
                | Self::Built(..)
                | Self::DownloadFailed(..)
                | Self::BuildFailed(..)
        )
    }
}

fn format_took(took: Duration) -> String {
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
    pub bin_dir: Option<PathBuf>,
    pub update_dir: PathBuf,
    pub public_key: &'static str,
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
        let config_dir = crate::config::dir()?;
        Some(Self {
            http: reqwest::Client::new(),
            releases_url: RELEASES_URL.into(),
            current_version: env!("CARGO_PKG_VERSION"),
            cargo: find_on_path("cargo", std::env::var_os("PATH")),
            rustc: find_on_path("rustc", std::env::var_os("PATH")),
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
            report(Progress::CargoMissing(version));
            return;
        };
        let folder = self.update_dir.join(format!(
            "{}-{}",
            std::process::id(),
            RUNS.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(error) = crate::config::create_private_dir(&self.update_dir)
            .and_then(|()| create_update_folder(&folder))
        {
            report(Progress::Failed(
                version,
                format!("creating {}: {error}", folder.display()),
            ));
            return;
        }
        let outcome = async {
            let asset_url = |name: &str| {
                release
                    .assets
                    .iter()
                    .find(|asset| asset.name == name)
                    .map(|asset| asset.browser_download_url.clone())
                    .with_context(|| format!("the release has no {name}"))
                    .map_err(|error| Progress::Failed(version.clone(), format!("{error:#}")))
            };
            let url = asset_url(ASSET)?;
            let signature_url = asset_url(SIGNATURE_ASSET)?;
            report(Progress::Downloading(version.clone()));
            let started = Instant::now();
            self.download(&url, &signature_url, &version, &folder)
                .await
                .map_err(|error| {
                    Progress::DownloadFailed(
                        version.clone(),
                        format!("{error:#}"),
                        started.elapsed(),
                    )
                })?;
            report(Progress::Downloaded(version.clone(), started.elapsed()));
            report(Progress::Building(version.clone()));
            let started = Instant::now();
            self.build(cargo, &folder).await.map_err(|error| {
                Progress::BuildFailed(version.clone(), format!("{error:#}"), started.elapsed())
            })?;
            report(Progress::Built(version.clone(), started.elapsed()));
            Ok(())
        }
        .await;
        let _ = std::fs::remove_dir_all(&folder);
        match outcome {
            Ok(()) => {
                if let Some(bin_dir) = &self.bin_dir {
                    link_alias(bin_dir);
                }
                report(Progress::Ready(version));
            }
            Err(failure) => report(failure),
        }
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header(reqwest::header::USER_AGENT, "rust-coding-agent")
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
            .to_vec())
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
        if signed_as != format!("rust-coding-agent {version}") {
            anyhow::bail!("{ASSET} is signed as {signed_as}, not {version}");
        }
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
            .arg(folder.join("rust-coding-agent"))
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
            .map_or_else(
                || format!("cargo install exited with {}", output.status),
                str::to_string,
            );
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

fn create_update_folder(folder: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(folder)
}

fn link_alias(bin_dir: &Path) {
    link_alias_to(bin_dir, std::ffi::OsStr::new("rust-coding-agent"));
}

fn link_alias_to(bin_dir: &Path, program: &std::ffi::OsStr) {
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
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))?;
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

pub fn find_on_path(program: &str, path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let file = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&path?)
        .map(|directory| directory.join(&file))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::{Progress, Updater, create_update_folder, find_on_path, link_alias_beside};
    use std::{
        io::Write,
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    struct TemporaryDir(PathBuf);

    impl std::ops::Deref for TemporaryDir {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TemporaryDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(name: &str) -> TemporaryDir {
        let dir = std::env::temp_dir().join(format!(
            "rust-coding-agent-update-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        TemporaryDir(dir)
    }

    fn source_zip() -> Vec<u8> {
        let mut buffer = std::io::Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("rust-coding-agent/Cargo.toml", options)
            .unwrap();
        zip.write_all(b"[package]\nname = \"rust-coding-agent\"\nversion = \"0.2.0\"\n")
            .unwrap();
        zip.finish().unwrap();
        buffer.into_inner()
    }

    const TEST_PUBLIC_KEY: &str = "RWSKhh96XMkv/ZkVrv3kVbTFGimqZdjSWovAPitGSQ6P6pMv5/H9UytL";
    const SIGNED_ZIP: &[u8] = include_bytes!("../tests/fixtures/update/rust-coding-agent.zip");
    const SIGNATURE: &[u8] =
        include_bytes!("../tests/fixtures/update/rust-coding-agent.zip.minisig");

    /// Serves the latest release on `/latest`, a zip signed with the test
    /// key on `/rust-coding-agent.zip` and its signature, and counts the
    /// requests.
    async fn github(tag: &str) -> (String, Arc<Mutex<Vec<String>>>) {
        github_serving(tag, SIGNED_ZIP.to_vec(), Some(SIGNATURE.to_vec())).await
    }

    async fn github_serving(
        tag: &str,
        zip: Vec<u8>,
        signature: Option<Vec<u8>>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut assets = vec![
            serde_json::json!({ "name": "rust-coding-agent.zip", "browser_download_url": format!("{base}/rust-coding-agent.zip") }),
        ];
        if signature.is_some() {
            assets.push(serde_json::json!({ "name": "rust-coding-agent.zip.minisig", "browser_download_url": format!("{base}/rust-coding-agent.zip.minisig") }));
        }
        let release = serde_json::json!({ "tag_name": tag, "assets": assets })
            .to_string()
            .into_bytes();
        let signature = signature.unwrap_or_default();
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
                let body = match path.as_str() {
                    "/rust-coding-agent.zip" => &zip,
                    "/rust-coding-agent.zip.minisig" => &signature,
                    _ => &release,
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
        let log = dir.join("cargo.log");
        write_script(
            dir.join("cargo"),
            format!(
                "#!/bin/sh\necho \"$@\" > '{log}'\nfor last; do :; done\ngrep -q 'version = \"0.2.0\"' \"$last/Cargo.toml\" && echo unpacked >> '{log}'\necho 'error: could not compile' >&2\nexit {exit_code}\n",
                log = log.display()
            ),
        )
    }

    fn fake_rustc(dir: &Path, version: &str) -> PathBuf {
        write_script(dir.join("rustc"), format!("#!/bin/sh\necho '{version}'\n"))
    }

    fn write_script(path: PathBuf, script: String) -> PathBuf {
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
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
            bin_dir: None,
            update_dir: std::env::temp_dir()
                .join(format!("rust-coding-agent-updates-{}", std::process::id())),
            public_key: TEST_PUBLIC_KEY,
        }
    }

    #[tokio::test]
    async fn downloads_and_builds_a_newer_release_and_offers_a_restart() {
        let dir = temp_dir("newer");
        let (base, _) = github("v0.2.0").await;
        let cargo = fake_cargo(&dir, 0);
        let shown = run(updater(&base, Some(cargo))).await;
        assert_eq!(shown.len(), 6, "{shown:?}");
        assert_eq!(shown[..2], ["update v0.2.0 available", "downloading"]);
        assert!(shown[2].starts_with("downloaded in "), "{shown:?}");
        assert_eq!(shown[3], "building");
        assert!(shown[4].starts_with("built in "), "{shown:?}");
        assert_eq!(shown[5], "installed, restart rust-coding-agent to use it");
        let log = std::fs::read_to_string(dir.join("cargo.log")).unwrap();
        assert!(log.starts_with("install --locked --force --path "), "{log}");
        assert!(log.contains("unpacked"), "{log}");
    }

    #[tokio::test]
    async fn links_rca_to_the_installed_binary() {
        let dir = temp_dir("link");
        let (base, _) = github("v0.2.0").await;
        let mut updater = updater(&base, Some(fake_cargo(&dir, 0)));
        updater.bin_dir = Some(dir.join("bin"));
        run(updater).await;
        assert_eq!(
            std::fs::read_link(dir.join("bin/rca")).unwrap(),
            Path::new("rust-coding-agent")
        );
    }

    #[tokio::test]
    async fn keeps_an_existing_rca() {
        let dir = temp_dir("existing-rca");
        let (base, _) = github("v0.2.0").await;
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/rca"), "other").unwrap();
        let mut updater = updater(&base, Some(fake_cargo(&dir, 0)));
        updater.bin_dir = Some(dir.join("bin"));
        run(updater).await;
        assert_eq!(
            std::fs::read_to_string(dir.join("bin/rca")).unwrap(),
            "other"
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_rca_beside_the_program_at_start() {
        let dir = temp_dir("start-link");
        let program = dir.join("rust-coding-agent");
        std::fs::write(&program, "").unwrap();
        link_alias_beside(&program, false);
        assert_eq!(
            std::fs::read_link(dir.join("rca")).unwrap(),
            Path::new("rust-coding-agent")
        );
    }

    #[test]
    fn does_not_link_rca_in_a_debug_build() {
        let dir = temp_dir("start-link-debug");
        let program = dir.join("rust-coding-agent");
        std::fs::write(&program, "").unwrap();
        link_alias_beside(&program, true);
        assert!(dir.join("rca").symlink_metadata().is_err());
    }

    #[test]
    fn keeps_an_existing_rca_beside_the_program() {
        let dir = temp_dir("start-link-existing");
        let program = dir.join("rust-coding-agent");
        std::fs::write(&program, "").unwrap();
        std::fs::write(dir.join("rca"), "other").unwrap();
        link_alias_beside(&program, false);
        assert_eq!(std::fs::read_to_string(dir.join("rca")).unwrap(), "other");
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
    async fn unpacks_the_release_in_the_update_folder() {
        let dir = temp_dir("update-folder");
        let (base, _) = github("v0.2.0").await;
        let mut updater = updater(&base, Some(fake_cargo(&dir, 0)));
        updater.update_dir = dir.join("updates");
        run(updater).await;
        let log = std::fs::read_to_string(dir.join("cargo.log")).unwrap();
        let path = log
            .lines()
            .next()
            .unwrap()
            .split(" --path ")
            .nth(1)
            .unwrap();
        assert!(Path::new(path).starts_with(dir.join("updates")), "{log}");
        assert!(log.contains("unpacked"), "{log}");
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
    async fn refuses_a_release_whose_signature_does_not_match() {
        let dir = temp_dir("bad-signature");
        let (base, _) = github_serving("v0.2.0", source_zip(), Some(SIGNATURE.to_vec())).await;
        let cargo = fake_cargo(&dir, 0);
        let shown = run(updater(&base, Some(cargo))).await;
        let last = shown.last().unwrap();
        assert!(
            last.starts_with("download failed after ")
                && last.ends_with("s: the signature of rust-coding-agent.zip is not valid"),
            "{shown:?}"
        );
        assert!(!dir.join("cargo.log").exists());
    }

    #[tokio::test]
    async fn refuses_a_release_signed_for_another_version() {
        let dir = temp_dir("other-version");
        let (base, _) = github("v0.3.0").await;
        let cargo = fake_cargo(&dir, 0);
        let shown = run(updater(&base, Some(cargo))).await;
        assert!(
            shown.last().unwrap().ends_with(
                "s: rust-coding-agent.zip is signed as rust-coding-agent v0.2.0, not v0.3.0"
            ),
            "{shown:?}"
        );
        assert!(!dir.join("cargo.log").exists());
    }

    #[tokio::test]
    async fn refuses_a_release_signed_with_another_key() {
        let dir = temp_dir("other-key");
        let (base, _) = github("v0.2.0").await;
        let cargo = fake_cargo(&dir, 0);
        let mut updater = updater(&base, Some(cargo));
        updater.public_key = super::PUBLIC_KEY;
        let shown = run(updater).await;
        assert!(
            shown
                .last()
                .unwrap()
                .ends_with("s: the signature of rust-coding-agent.zip is not valid"),
            "{shown:?}"
        );
        assert!(!dir.join("cargo.log").exists());
    }

    #[tokio::test]
    async fn refuses_a_release_without_a_signature() {
        let dir = temp_dir("unsigned");
        let (base, _) = github_serving("v0.2.0", SIGNED_ZIP.to_vec(), None).await;
        let cargo = fake_cargo(&dir, 0);
        let shown = run(updater(&base, Some(cargo))).await;
        assert_eq!(
            shown,
            [
                "update v0.2.0 available",
                "failed: the release has no rust-coding-agent.zip.minisig"
            ]
        );
        assert!(!dir.join("cargo.log").exists());
    }

    #[tokio::test]
    async fn shows_why_building_failed() {
        let dir = temp_dir("failed");
        let (base, _) = github("v0.2.0").await;
        let cargo = fake_cargo(&dir, 101);
        let shown = run(updater(&base, Some(cargo))).await;
        let last = shown.last().unwrap();
        assert!(
            last.starts_with("build failed after ")
                && last.ends_with("s: error: could not compile"),
            "{shown:?}"
        );
    }

    #[test]
    fn shows_how_long_each_step_took() {
        let seconds = std::time::Duration::from_secs;
        let built = |took| Progress::Built("v0.2.0".into(), seconds(took)).message();
        assert_eq!(built(0), "built in 0s");
        assert_eq!(built(42), "built in 42s");
        assert_eq!(built(125), "built in 2m 5s");
        assert_eq!(
            Progress::Downloaded("v0.2.0".into(), seconds(3)).message(),
            "downloaded in 3s"
        );
        assert_eq!(
            Progress::DownloadFailed("v0.2.0".into(), "error: x".into(), seconds(3)).message(),
            "download failed after 3s: error: x"
        );
        assert_eq!(
            Progress::BuildFailed("v0.2.0".into(), "error: x".into(), seconds(70)).message(),
            "build failed after 1m 10s: error: x"
        );
        assert_eq!(
            Progress::Failed("v0.2.0".into(), "no zip".into()).message(),
            "failed: no zip"
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
                "cargo not found on PATH, install Rust to update",
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
        if cfg!(debug_assertions) {
            assert!(Updater::for_this_build(&settings).is_none());
        }
    }

    #[test]
    fn creates_a_new_update_folder_only_the_user_can_open() {
        let dir = temp_dir("update-folder");
        let folder = dir.join("update");
        create_update_folder(&folder).unwrap();
        let mode = crate::config::permissions(&folder);
        let again = create_update_folder(&folder);
        assert_eq!(mode, Some(0o700));
        assert_eq!(again.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn finds_programs_on_path() {
        let dir = temp_dir("path");
        let cargo = fake_cargo(&dir, 0);
        let path = std::env::join_paths([dir.join("missing"), dir.to_path_buf()]).unwrap();
        assert_eq!(find_on_path("cargo", Some(path.clone())), Some(cargo));
        assert_eq!(find_on_path("rustc", Some(path)), None);
        assert_eq!(find_on_path("cargo", None), None);
    }
}
