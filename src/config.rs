use std::{path::PathBuf, sync::OnceLock};

static CONFIG_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_dir(path: PathBuf) {
    let _ = CONFIG_DIR.set(path);
}

pub fn dir() -> Option<PathBuf> {
    if let Some(path) = CONFIG_DIR.get() {
        return Some(path.clone());
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rust-claude"))
}
