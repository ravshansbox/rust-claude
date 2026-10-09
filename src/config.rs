use std::{path::PathBuf, sync::OnceLock};

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_dir(path: PathBuf) {
    let _ = CONFIG_DIR.set(path);
}

pub fn dir() -> Option<PathBuf> {
    if let Some(path) = CONFIG_DIR.get() {
        return Some(path.clone());
    }
    default_dir()
}

#[cfg(not(test))]
fn default_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rust-claude"))
}

#[cfg(test)]
fn default_dir() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("rust-claude-test-config-{}", std::process::id())))
}
