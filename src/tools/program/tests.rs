use super::on_path;
use crate::tools::test_support::TemporaryDir;
use std::{ffi::OsString, os::unix::fs::PermissionsExt};

#[test]
fn finds_programs_on_the_path() {
    let empty = TemporaryDir::new("path-empty");
    let installed = TemporaryDir::new("path-installed");
    let program = installed.path().join("ast-grep");
    std::fs::write(&program, "").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path =
        |dirs: &[&TemporaryDir]| std::env::join_paths(dirs.iter().map(|dir| dir.path())).unwrap();
    assert!(on_path("ast-grep", Some(path(&[&empty, &installed]))));
    assert!(!on_path("ast-grep", Some(path(&[&empty]))));
    assert!(!on_path("ast-grep", None::<OsString>));
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!on_path("ast-grep", Some(path(&[&installed]))));
}
