use super::*;

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
