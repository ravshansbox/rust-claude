use super::{Progress, Settings, Updater, create_update_folder, find_on_path, link_alias_beside};
use crate::tools::test_support::TemporaryDir;
use std::{
    io::{Cursor, ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn source_zip() -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
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
const SIGNED_ZIP: &[u8] = include_bytes!("../../tests/fixtures/update/rust-coding-agent.zip");
const SIGNATURE: &[u8] =
    include_bytes!("../../tests/fixtures/update/rust-coding-agent.zip.minisig");

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
            let request = String::from_utf8_lossy(&data).into_owned();
            let body = match request.split_whitespace().nth(1) {
                Some("/rust-coding-agent.zip") => &zip,
                Some("/rust-coding-agent.zip.minisig") => &signature,
                _ => &release,
            };
            recorded.lock().unwrap().push(request);
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
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(&path, script).unwrap();
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
    let dir = TemporaryDir::new("update-newer");
    let (base, _) = github("v0.2.0").await;
    let cargo = fake_cargo(dir.path(), 0);
    let shown = run(updater(&base, Some(cargo))).await;
    assert_eq!(shown.len(), 6, "{shown:?}");
    assert_eq!(shown[..2], ["update v0.2.0 available", "downloading"]);
    assert!(shown[2].starts_with("downloaded in "), "{shown:?}");
    assert_eq!(shown[3], "building");
    assert!(shown[4].starts_with("built in "), "{shown:?}");
    assert_eq!(shown[5], "installed, restart rust-coding-agent to use it");
    let log = std::fs::read_to_string(dir.path().join("cargo.log")).unwrap();
    assert!(log.starts_with("install --locked --force --path "), "{log}");
    assert!(log.contains("unpacked"), "{log}");
}

#[tokio::test]
async fn links_rca_to_the_installed_binary() {
    let dir = TemporaryDir::new("update-link");
    let (base, _) = github("v0.2.0").await;
    let mut updater = updater(&base, Some(fake_cargo(dir.path(), 0)));
    updater.bin_dir = Some(dir.path().join("bin"));
    run(updater).await;
    assert_eq!(
        std::fs::read_link(dir.path().join("bin/rca")).unwrap(),
        Path::new("rust-coding-agent")
    );
}

#[cfg(unix)]
#[test]
fn links_rca_beside_the_program_at_start() {
    let dir = TemporaryDir::new("update-start-link");
    let program = dir.path().join("rust-coding-agent");
    std::fs::write(&program, "").unwrap();
    link_alias_beside(&program, false);
    assert_eq!(
        std::fs::read_link(dir.path().join("rca")).unwrap(),
        Path::new("rust-coding-agent")
    );
}

#[test]
fn does_not_link_rca_in_a_debug_build() {
    let dir = TemporaryDir::new("update-start-link-debug");
    let program = dir.path().join("rust-coding-agent");
    std::fs::write(&program, "").unwrap();
    link_alias_beside(&program, true);
    assert!(dir.path().join("rca").symlink_metadata().is_err());
}

#[test]
fn keeps_an_existing_rca_beside_the_program() {
    let dir = TemporaryDir::new("update-start-link-existing");
    let program = dir.path().join("rust-coding-agent");
    std::fs::write(&program, "").unwrap();
    std::fs::write(dir.path().join("rca"), "other").unwrap();
    link_alias_beside(&program, false);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("rca")).unwrap(),
        "other"
    );
}

#[tokio::test]
async fn builds_in_the_build_folder() {
    let dir = TemporaryDir::new("update-build-folder");
    let (base, _) = github("v0.2.0").await;
    let mut updater = updater(&base, Some(fake_cargo(dir.path(), 0)));
    updater.build_dir = Some(dir.path().join("build"));
    run(updater).await;
    let log = std::fs::read_to_string(dir.path().join("cargo.log")).unwrap();
    let expected = format!(
        "install --locked --force --target-dir {} --path ",
        dir.path().join("build").display()
    );
    assert!(log.starts_with(&expected), "{log}");
}

#[tokio::test]
async fn unpacks_the_release_in_the_update_folder() {
    let dir = TemporaryDir::new("update-update-folder");
    let (base, _) = github("v0.2.0").await;
    let mut updater = updater(&base, Some(fake_cargo(dir.path(), 0)));
    updater.update_dir = dir.path().join("updates");
    run(updater).await;
    let log = std::fs::read_to_string(dir.path().join("cargo.log")).unwrap();
    let path = log
        .lines()
        .next()
        .unwrap()
        .split(" --path ")
        .nth(1)
        .unwrap();
    assert!(
        Path::new(path).starts_with(dir.path().join("updates")),
        "{log}"
    );
    assert!(log.contains("unpacked"), "{log}");
}

#[tokio::test]
async fn keeps_the_build_folder_while_the_compiler_stays_the_same() {
    let dir = TemporaryDir::new("update-same-compiler");
    let (base, _) = github("v0.2.0").await;
    let build = dir.path().join("build");
    for _ in 0..2 {
        let mut updater = updater(&base, Some(fake_cargo(dir.path(), 0)));
        updater.rustc = Some(fake_rustc(dir.path(), "rustc 1.99.0"));
        updater.build_dir = Some(build.clone());
        run(updater).await;
        std::fs::write(build.join("earlier-build"), "").unwrap();
    }
    assert!(build.join("earlier-build").exists());
}

#[tokio::test]
async fn empties_the_build_folder_when_the_compiler_changes() {
    let dir = TemporaryDir::new("update-new-compiler");
    let (base, _) = github("v0.2.0").await;
    let build = dir.path().join("build");
    for version in ["rustc 1.98.0", "rustc 1.99.0"] {
        let mut updater = updater(&base, Some(fake_cargo(dir.path(), 0)));
        updater.rustc = Some(fake_rustc(dir.path(), version));
        updater.build_dir = Some(build.clone());
        run(updater).await;
        std::fs::write(build.join(version), "").unwrap();
    }
    assert!(!build.join("rustc 1.98.0").exists());
    assert!(build.join("rustc 1.99.0").exists());
}

#[tokio::test]
async fn builds_outside_the_project_it_was_started_in() {
    let dir = TemporaryDir::new("update-outside-project");
    let (base, _) = github("v0.2.0").await;
    let mut updater = updater(&base, None);
    updater.cargo = Some(write_script(
        dir.path().join("cargo"),
        format!(
            "#!/bin/sh\npwd -P > '{}'\n",
            dir.path().join("cargo.pwd").display()
        ),
    ));
    updater.rustc = Some(write_script(
        dir.path().join("rustc"),
        format!(
            "#!/bin/sh\npwd -P > '{}'\necho 'rustc 1.99.0'\n",
            dir.path().join("rustc.pwd").display()
        ),
    ));
    updater.build_dir = Some(dir.path().join("build"));
    updater.update_dir = dir.path().join("updates");
    run(updater).await;
    let updates = dir.path().canonicalize().unwrap().join("updates");
    for program in ["cargo", "rustc"] {
        let pwd = std::fs::read_to_string(dir.path().join(format!("{program}.pwd"))).unwrap();
        assert!(
            Path::new(pwd.trim_end()).starts_with(&updates),
            "{program} ran in {pwd}"
        );
    }
}

#[tokio::test]
async fn refuses_a_release_whose_signature_does_not_match() {
    let dir = TemporaryDir::new("update-bad-signature");
    let (base, _) = github_serving("v0.2.0", source_zip(), Some(SIGNATURE.to_vec())).await;
    let cargo = fake_cargo(dir.path(), 0);
    let shown = run(updater(&base, Some(cargo))).await;
    let last = shown.last().unwrap();
    assert!(
        last.starts_with("download failed after ")
            && last.ends_with("s: the signature of rust-coding-agent.zip is not valid"),
        "{shown:?}"
    );
    assert!(!dir.path().join("cargo.log").exists());
}

#[tokio::test]
async fn refuses_a_release_signed_for_another_version() {
    let dir = TemporaryDir::new("update-other-version");
    let (base, _) = github("v0.3.0").await;
    let cargo = fake_cargo(dir.path(), 0);
    let shown = run(updater(&base, Some(cargo))).await;
    assert!(
        shown.last().unwrap().ends_with(
            "s: rust-coding-agent.zip is signed as rust-coding-agent v0.2.0, not v0.3.0"
        ),
        "{shown:?}"
    );
    assert!(!dir.path().join("cargo.log").exists());
}

#[tokio::test]
async fn refuses_a_release_signed_with_another_key() {
    let dir = TemporaryDir::new("update-other-key");
    let (base, _) = github("v0.2.0").await;
    let cargo = fake_cargo(dir.path(), 0);
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
    assert!(!dir.path().join("cargo.log").exists());
}

#[tokio::test]
async fn refuses_a_release_without_a_signature() {
    let dir = TemporaryDir::new("update-unsigned");
    let (base, _) = github_serving("v0.2.0", SIGNED_ZIP.to_vec(), None).await;
    let cargo = fake_cargo(dir.path(), 0);
    let shown = run(updater(&base, Some(cargo))).await;
    assert_eq!(
        shown,
        [
            "update v0.2.0 available",
            "failed: the release has no rust-coding-agent.zip.minisig"
        ]
    );
    assert!(!dir.path().join("cargo.log").exists());
}

#[tokio::test]
async fn shows_why_building_failed() {
    let dir = TemporaryDir::new("update-failed");
    let (base, _) = github("v0.2.0").await;
    let cargo = fake_cargo(dir.path(), 101);
    let shown = run(updater(&base, Some(cargo))).await;
    let last = shown.last().unwrap();
    assert!(
        last.starts_with("build failed after ") && last.ends_with("s: error: could not compile"),
        "{shown:?}"
    );
}

#[tokio::test]
async fn stops_cargo_and_deletes_the_update_folder_when_quit_while_building() {
    let dir = TemporaryDir::new("update-quit");
    let (base, _) = github("v0.2.0").await;
    let started = dir.path().join("started");
    let finished = dir.path().join("finished");
    let mut updater = updater(&base, None);
    updater.cargo = Some(write_script(
        dir.path().join("cargo"),
        format!(
            "#!/bin/sh\ntouch '{}'\nsleep 1\ntouch '{}'\n",
            started.display(),
            finished.display()
        ),
    ));
    updater.update_dir = dir.path().join("updates");
    let update = tokio::spawn(run(updater));
    while !started.exists() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    update.abort();
    assert!(update.await.unwrap_err().is_cancelled());
    assert_eq!(
        std::fs::read_dir(dir.path().join("updates"))
            .unwrap()
            .count(),
        0
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!finished.exists());
}

#[test]
fn shows_how_long_each_step_took() {
    let seconds = Duration::from_secs;
    let built = |took| Progress::Built(seconds(took)).message();
    assert_eq!(built(0), "built in 0s");
    assert_eq!(built(42), "built in 42s");
    assert_eq!(built(125), "built in 2m 5s");
    assert_eq!(
        Progress::Downloaded(seconds(3)).message(),
        "downloaded in 3s"
    );
    assert_eq!(
        Progress::DownloadFailed("error: x".into(), seconds(3)).message(),
        "download failed after 3s: error: x"
    );
    assert_eq!(
        Progress::BuildFailed("error: x".into(), seconds(70)).message(),
        "build failed after 1m 10s: error: x"
    );
    assert_eq!(
        Progress::Failed("no zip".into()).message(),
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
async fn stays_quiet_unless_the_latest_release_is_a_newer_version() {
    for tag in ["v0.1.0", "v0.0.9", "v0.3.0-rc.1", "v0.3", "v0.3.0.1"] {
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
async fn skips_builds_from_main() {
    let (base, requests) = github("v0.2.0").await;
    let mut updater = updater(&base, None);
    updater.current_version = "0.0.0";
    assert!(run(updater).await.is_empty());
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn can_be_turned_off_in_settings() {
    let settings = |check_for_updates| Settings {
        check_for_updates,
        ..Default::default()
    };
    assert!(Updater::new(&settings(Some(false)), false).is_none());
    assert!(Updater::new(&settings(Some(true)), false).is_some());
    assert!(Updater::new(&settings(None), false).is_some());
}

#[test]
fn skips_debug_builds_such_as_cargo_run() {
    let settings = Settings {
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
    use crate::config::test_support::permissions;

    let dir = TemporaryDir::new("update-update-folder");
    let folder = dir.path().join("update");
    create_update_folder(&folder).unwrap();
    let mode = permissions(&folder);
    let again = create_update_folder(&folder);
    assert_eq!(mode, Some(0o700));
    assert_eq!(again.unwrap_err().kind(), ErrorKind::AlreadyExists);
}

#[test]
fn finds_programs_on_path() {
    let dir = TemporaryDir::new("update-path");
    let cargo = fake_cargo(dir.path(), 0);
    let path =
        std::env::join_paths([dir.path().join("missing"), dir.path().to_path_buf()]).unwrap();
    assert_eq!(find_on_path("cargo", Some(path.clone())), Some(cargo));
    assert_eq!(find_on_path("rustc", Some(path)), None);
    assert_eq!(find_on_path("cargo", None), None);
}
