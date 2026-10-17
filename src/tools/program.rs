use std::{ffi::OsString, path::Path};

pub(super) fn installed(program: &str) -> bool {
    on_path(program, std::env::var_os("PATH"))
}

fn on_path(program: &str, path: Option<OsString>) -> bool {
    path.is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| is_executable(&dir.join(program)))
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.with_extension("exe").is_file()
}

#[cfg(test)]
mod tests;
