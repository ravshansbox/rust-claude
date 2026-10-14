use super::*;

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
    updater.public_key = super::super::PUBLIC_KEY;
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
