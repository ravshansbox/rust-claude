use super::*;

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
