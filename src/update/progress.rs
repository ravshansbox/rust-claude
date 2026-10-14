use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Available(String),
    CargoMissing,
    Downloading,
    Downloaded(Duration),
    Building,
    Built(Duration),
    Ready,
    DownloadFailed(String, Duration),
    BuildFailed(String, Duration),
    Failed(String),
}

impl Progress {
    pub fn message(&self) -> String {
        match self {
            Self::Available(version) => format!("update {version} available"),
            Self::CargoMissing => "cargo not found on PATH, install Rust to update".into(),
            Self::Downloading => "downloading".into(),
            Self::Downloaded(took) => format!("downloaded in {}", format_took(*took)),
            Self::Building => "building".into(),
            Self::Built(took) => format!("built in {}", format_took(*took)),
            Self::Ready => "installed, restart rust-coding-agent to use it".into(),
            Self::DownloadFailed(error, took) => {
                format!("download failed after {}: {error}", format_took(*took))
            }
            Self::BuildFailed(error, took) => {
                format!("build failed after {}: {error}", format_took(*took))
            }
            Self::Failed(error) => format!("failed: {error}"),
        }
    }

    pub fn finishes_step(&self) -> bool {
        matches!(
            self,
            Self::Downloaded(..)
                | Self::Built(..)
                | Self::DownloadFailed(..)
                | Self::BuildFailed(..)
        )
    }
}

fn format_took(took: Duration) -> String {
    let seconds = took.as_secs();
    match seconds / 60 {
        0 => format!("{seconds}s"),
        minutes => format!("{minutes}m {}s", seconds % 60),
    }
}
