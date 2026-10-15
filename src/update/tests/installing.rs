use super::*;

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
