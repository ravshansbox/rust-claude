use std::{io::ErrorKind, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;

#[derive(Default, Serialize, Deserialize)]
pub struct Settings {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub check_for_updates: Option<bool>,
    pub notifications: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Settings {
    /// Reads settings.json, or the defaults when it is missing. When it
    /// cannot be read, update checks are off, so a typo cannot turn them on.
    pub fn load() -> Self {
        config::file("settings.json")
            .map(|path| load_file(&path))
            .unwrap_or_default()
    }

    /// Applies `change` to settings.json, keeping the settings it leaves
    /// alone. Fails instead of replacing a file it cannot read.
    pub fn update(change: impl FnOnce(&mut Self)) -> Result<()> {
        update_file(&config::file("settings.json")?, change)
    }
}

fn load_file(path: &Path) -> Settings {
    read_file(path).unwrap_or_else(|_| Settings {
        check_for_updates: Some(false),
        ..Settings::default()
    })
}

fn read_file(path: &Path) -> Result<Settings> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Settings::default()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    serde_json::from_str(&text).map_err(|error| {
        anyhow::anyhow!(
            "{} could not be read: {error}; fix or delete it",
            path.display()
        )
    })
}

fn update_file(path: &Path, change: impl FnOnce(&mut Settings)) -> Result<()> {
    let mut settings = read_file(path)?;
    change(&mut settings);
    config::create_private_dir(path.parent().context("invalid settings path")?)?;
    config::write_private_file(path, serde_json::to_string_pretty(&settings)?.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests;
