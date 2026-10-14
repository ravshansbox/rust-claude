use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::tui) enum Activity {
    Working,
    Compacting,
    Running,
    Cancelling,
    Resuming,
    StartingNewSession,
    LoadingSessions,
}

impl Activity {
    /// Whether Esc can stop it.
    pub(in crate::tui) fn can_cancel(self) -> bool {
        matches!(self, Self::Working | Self::Compacting | Self::Running)
    }

    /// Whether prompts sent meanwhile are queued for the model.
    pub(super) fn can_queue(self) -> bool {
        matches!(self, Self::Working | Self::Compacting)
    }
}

impl fmt::Display for Activity {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(match self {
            Self::Working => "working",
            Self::Compacting => "compacting",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Resuming => "resuming",
            Self::StartingNewSession => "starting new session",
            Self::LoadingSessions => "loading sessions",
        })
    }
}
