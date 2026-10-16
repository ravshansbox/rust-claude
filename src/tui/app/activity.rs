use super::App;
use crate::tui::{keys::Action, message::Role};
use anyhow::Result;
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

impl App {
    pub(in crate::tui) fn busy(&self) -> bool {
        self.activity.is_some()
    }

    pub(in crate::tui) fn animating(&self) -> bool {
        self.busy() || self.mcp_loading()
    }

    pub(in crate::tui) fn tick_spinner(&mut self) {
        self.spinner_frame = self.spinner_frame.wrapping_add(1);
        self.show_loading_mcp_groups();
    }

    pub(in crate::tui) fn submit(&mut self, prompt: String) -> Action {
        self.remember(&prompt);
        self.start(Activity::Working);
        let images = self.take_numbered_images(&prompt);
        let images: Vec<_> = images.into_iter().map(|(_, image)| image).collect();
        if self.start_loop(&prompt, &images) {
            return Action::SubmitLoop(prompt, images, self.effort);
        }
        Action::Submit(prompt, images, self.effort)
    }

    pub(in crate::tui) fn start(&mut self, activity: Activity) {
        self.activity = Some(activity);
    }

    pub(in crate::tui) fn finish<T>(
        &mut self,
        result: Result<T>,
        on_success: impl FnOnce(&mut Self, T),
    ) {
        if let Some(last) = self.messages.last_mut() {
            last.finish_streaming();
        }
        match result {
            Ok(value) => on_success(self, value),
            Err(error) => self.push(Role::Event, format!("error: {error:#}")),
        }
        self.activity = None;
    }
}
