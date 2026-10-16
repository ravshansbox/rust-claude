use std::time::Duration;

use anyhow::Result;
use tokio::signal::unix::{Signal, SignalKind, signal};

use crate::auth;

/// Ctrl+C, a closed terminal (hangup) and `kill` (terminate) all stop
/// rust-coding-agent the same way, so running commands and MCP servers are stopped
/// instead of left behind.
pub struct StopSignals {
    interrupt: Signal,
    hangup: Signal,
    terminate: Signal,
}

impl StopSignals {
    pub fn new() -> Result<Self> {
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            hangup: signal(SignalKind::hangup())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// Waits for a stop signal and returns the conventional exit status for
    /// it: 128 plus the signal number.
    pub async fn recv(&mut self) -> i32 {
        tokio::select! {
            _ = self.interrupt.recv() => INTERRUPTED,
            _ = self.hangup.recv() => 129,
            _ = self.terminate.recv() => 143,
        }
    }
}

/// The exit status after Ctrl+C.
pub const INTERRUPTED: i32 = 130;

/// How long stopping waits for a sign-in renewal to be saved.
pub const RENEWAL_WAIT: Duration = Duration::from_secs(5);

/// Exits after a stop signal, once a sign-in renewal in progress is saved.
pub async fn stopped(status: i32) -> Result<()> {
    auth::finish_renewal(RENEWAL_WAIT).await;
    eprintln!("\ncancelled");
    std::process::exit(status);
}
