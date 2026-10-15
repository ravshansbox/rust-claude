use super::{Activity, App};
use crate::{
    agent::{Queued, take_queued},
    images::Image,
    tui::message::Role,
};

impl App {
    pub(in crate::tui) fn can_queue(&self) -> bool {
        self.activity.is_some_and(Activity::can_queue)
    }

    pub(in crate::tui) fn queue_prompt(&mut self) {
        let prompt = self.take_input();
        self.remember(&prompt);
        let images = self.take_numbered_images(&prompt);
        if let Ok(mut queue) = self.queue.lock() {
            queue.push(Queued { prompt, images });
        }
    }

    pub(in crate::tui) fn send_queued(&mut self) -> Option<(String, Vec<Image>)> {
        let queued = take_queued(&self.queue);
        if queued.is_empty() {
            return None;
        }
        let prompt = queued
            .iter()
            .map(|queued| queued.prompt.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let images = queued
            .into_iter()
            .flat_map(|queued| queued.images)
            .map(|(_, image)| image)
            .collect();
        self.push(Role::User, prompt.clone());
        self.start(Activity::Working);
        Some((prompt, images))
    }

    pub(in crate::tui) fn restore_queued(&mut self) {
        let queued = take_queued(&self.queue);
        if queued.is_empty() {
            return;
        }
        let mut parts = Vec::new();
        let mut images = Vec::new();
        for queued in queued {
            parts.push(queued.prompt);
            images.extend(queued.images);
        }
        images.append(&mut self.images);
        self.images = images;
        if !self.input.is_empty() {
            parts.push(std::mem::take(&mut self.input));
        }
        self.restore_input(parts.join("\n\n"));
    }
}
