use std::{ffi::OsStr, path::Path};

pub(super) fn link_alias_to(bin_dir: &Path, program: &OsStr) {
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

pub(super) fn link_alias_beside(program: &Path, debug_build: bool) {
    if debug_build {
        return;
    }
    if let (Some(dir), Some(name)) = (program.parent(), program.file_name()) {
        link_alias_to(dir, name);
    }
}
