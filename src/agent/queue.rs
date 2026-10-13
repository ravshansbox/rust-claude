use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::{Value, json};

use super::{Agent, AgentEvent};
use crate::images::Image;

pub struct Queued {
    pub prompt: String,
    pub images: Vec<(usize, Image)>,
}

pub type Queue = Arc<Mutex<Vec<Queued>>>;

pub fn take_queued(queue: &Queue) -> Vec<Queued> {
    queue
        .lock()
        .map(|mut queued| std::mem::take(&mut *queued))
        .unwrap_or_default()
}

impl Agent {
    /// Turns queued prompts into message blocks. The prompts stay queued when
    /// saving one of their images fails. Images are saved with the queue
    /// unlocked, so the interface does not wait to draw or queue a prompt.
    pub(super) fn take_queued_prompts(
        &self,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Option<Vec<Value>>> {
        let queued = take_queued(&self.queue);
        if queued.is_empty() {
            return Ok(None);
        }
        let mut blocks = Vec::new();
        let saved = queued.iter().try_for_each(|queued| {
            for (_, image) in &queued.images {
                blocks.push(self.session.save_image(image)?);
            }
            blocks.push(json!({ "type": "text", "text": queued.prompt }));
            Ok::<_, anyhow::Error>(())
        });
        if let Err(error) = saved {
            // Ahead of any prompt queued meanwhile, keeping their order.
            if let Ok(mut queue) = self.queue.lock() {
                queue.splice(0..0, queued);
            }
            return Err(error);
        }
        for queued in queued {
            on_event(AgentEvent::Queued(queued.prompt));
        }
        Ok(Some(blocks))
    }
}
