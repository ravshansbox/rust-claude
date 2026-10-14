use super::App;
use crate::images::Image;

impl App {
    pub(in crate::tui) fn input_changed(&mut self) {
        self.history_index = None;
        self.command_selected = 0;
        self.commands_dismissed = false;
    }

    pub(in crate::tui) fn attach_image(&mut self, image: Image) {
        self.image_count += 1;
        self.insert(&image_marker(self.image_count));
        self.images.push((self.image_count, image));
    }

    pub(in crate::tui) fn take_numbered_images(&mut self, prompt: &str) -> Vec<(usize, Image)> {
        std::mem::take(&mut self.images)
            .into_iter()
            .filter(|(number, _)| prompt.contains(&image_marker(*number)))
            .collect()
    }

    /// Puts back text the user entered so they can fix it.
    pub(in crate::tui) fn take_input(&mut self) -> String {
        self.cursor = 0;
        self.forget_files();
        self.history_index = None;
        std::mem::take(&mut self.input)
    }

    pub(in crate::tui) fn insert(&mut self, text: &str) {
        self.input.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.input_changed();
    }

    pub(in crate::tui) fn delete_to(&mut self, start: usize) {
        self.input.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.input_changed();
    }

    pub(in crate::tui) fn restore_input(&mut self, text: String) {
        self.input = text;
        self.cursor = self.input.len();
        self.input_changed();
    }
}

fn image_marker(number: usize) -> String {
    format!("[image {number}]")
}
