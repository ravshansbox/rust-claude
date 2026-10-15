use std::{
    fs::{File, TryLockError},
    path::Path,
    time::Duration,
};

use anyhow::Result;

use crate::config;

/// Opens a session file for appending, locked until it is closed.
pub(super) fn open_locked(path: &Path, create: bool) -> Result<File> {
    let file = config::private_file()
        .create(create)
        .read(true)
        .append(true)
        .open(path)?;
    // A command started while the file was open shares the lock until the
    // command takes over, so a lock just released can look held for a moment.
    let mut attempts = 0;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if attempts < 20 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(TryLockError::WouldBlock) => {
                anyhow::bail!("this session is open in another rust-coding-agent")
            }
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
    }
}
