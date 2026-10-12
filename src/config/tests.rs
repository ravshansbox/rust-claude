use super::{WRITES, permissions, replace_file, write_private_file};
use crate::tools::test_support::TemporaryDir;
use std::sync::atomic::Ordering;

#[cfg(unix)]
#[test]
fn saves_through_symlinked_file() {
    let directory = TemporaryDir::new("linked-save");
    let base = directory.path();
    let target = base.join("dotfiles-settings.json");
    let link = base.join("settings.json");
    std::fs::write(&target, "{}").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let result = write_private_file(&link, b"{\"model\":\"opus\"}");
    let found = (
        std::fs::symlink_metadata(&link).unwrap().is_symlink(),
        std::fs::read_to_string(&target).unwrap(),
        permissions(&target),
    );
    result.unwrap();
    assert_eq!(found, (true, "{\"model\":\"opus\"}".into(), Some(0o600)));
}

/// A link to a file not made yet stays, and the file is made.
#[cfg(unix)]
#[test]
fn saves_through_dangling_symlink() {
    let directory = TemporaryDir::new("dangling-save");
    let base = directory.path();
    std::fs::create_dir(base.join("dotfiles")).unwrap();
    let link = base.join("settings.json");
    std::os::unix::fs::symlink("dotfiles/settings.json", &link).unwrap();
    let result = write_private_file(&link, b"{}");
    let found = (
        std::fs::symlink_metadata(&link).unwrap().is_symlink(),
        std::fs::read_to_string(base.join("dotfiles/settings.json")).ok(),
    );
    result.unwrap();
    assert_eq!(found, (true, Some("{}".into())));
}

/// A file someone links to from the next temporary names is left alone.
#[cfg(unix)]
#[test]
fn does_not_follow_symlinks_at_temporary_names() {
    let directory = TemporaryDir::new("planted");
    let base = directory.path();
    let victim = base.join("victim");
    std::fs::write(&victim, "secret").unwrap();
    let path = base.join("file.log");
    let next = WRITES.load(Ordering::Relaxed);
    for counter in next..next + 64 {
        let planted = base.join(format!("file.log.{}.{counter}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, planted).unwrap();
    }
    let result = replace_file(&path, b"new", std::fs::OpenOptions::new(), |_| Ok(()));
    let found = (
        std::fs::read_to_string(&victim).unwrap(),
        std::fs::read_to_string(&path).ok(),
        std::fs::symlink_metadata(&path)
            .map(|metadata| metadata.is_symlink())
            .ok(),
    );
    result.unwrap();
    assert_eq!(found, ("secret".into(), Some("new".into()), Some(false)));
}
