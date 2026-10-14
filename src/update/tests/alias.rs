use super::super::alias::link_alias_beside;
use super::*;

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
