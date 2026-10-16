use super::{Activity, App};
use crate::{images::Image, tui::message::Role};

#[derive(Default)]
pub(in crate::tui) struct Loop {
    prompt: Option<(String, Vec<Image>)>,
    round: usize,
}

impl App {
    pub(in crate::tui) fn toggle_loop(&mut self) {
        if self.looping.take().is_some() {
            self.push(Role::Event, "loop mode off");
        } else {
            self.looping = Some(Loop::default());
            self.push(Role::Event, "loop mode on");
        }
    }

    pub(in crate::tui) fn stop_loop(&mut self) {
        if self.looping.is_some() {
            self.toggle_loop();
        }
    }

    /// Keeps the first prompt sent in loop mode to send again after each
    /// run, and returns whether it is that prompt.
    pub(super) fn start_loop(&mut self, prompt: &str, images: &[Image]) -> bool {
        let Some(looping) = &mut self.looping else {
            return false;
        };
        if looping.prompt.is_some() {
            return false;
        }
        looping.prompt = Some((prompt.to_string(), images.to_vec()));
        looping.round = 1;
        self.clear_session();
        self.push(Role::User, prompt);
        true
    }

    /// Starts the next run of the loop in a new session once no reply,
    /// background command or queued prompt is waiting.
    pub(in crate::tui) fn next_loop_round(&mut self) -> Option<(String, Vec<Image>)> {
        let queued = self.queue.lock().map_or(0, |queue| queue.len());
        if self.busy() || self.stats.background > 0 || queued > 0 {
            return None;
        }
        let looping = self.looping.as_mut()?;
        let (prompt, images) = looping.prompt.clone()?;
        looping.round += 1;
        self.clear_session();
        self.push(Role::User, prompt.clone());
        self.start(Activity::Working);
        Some((prompt, images))
    }

    pub(in crate::tui) fn loop_label(&self) -> Option<String> {
        let looping = self.looping.as_ref()?;
        Some(match looping.round {
            0 => "loop".to_string(),
            round => format!("loop {round}"),
        })
    }
}
