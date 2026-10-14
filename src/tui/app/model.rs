use super::App;
use crate::{
    models,
    tui::{display_model, message::Role},
};

impl App {
    pub(in crate::tui) fn cycle_effort(&mut self) {
        let levels = models::effort_levels(&self.model);
        let index = levels
            .iter()
            .position(|level| *level == self.effort)
            .map_or(0, |index| (index + 1) % levels.len());
        self.effort = levels[index];
    }

    /// Returns whether `name` is an effort level.
    pub(in crate::tui) fn set_effort(&mut self, name: &str) -> bool {
        if let Some(level) = models::find_effort(&self.model, name) {
            self.effort = level;
            self.push(Role::Event, format!("effort: {level}"));
            true
        } else {
            self.push(
                Role::Event,
                format!(
                    "unknown effort level: {name} (options: {})",
                    models::effort_levels(&self.model).join(", ")
                ),
            );
            false
        }
    }

    pub(in crate::tui) fn set_model(&mut self, model: &str) {
        self.model = model.into();
        self.effort = models::supported_effort(model, self.effort);
        self.push(Role::Event, format!("model: {}", display_model(model)));
    }
}
