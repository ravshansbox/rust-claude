use super::App;

impl App {
    pub(in crate::tui) fn scroll_up(&mut self, amount: usize) {
        self.scroll_from_bottom = self
            .scroll_from_bottom
            .saturating_add(amount)
            .min(self.max_scroll);
    }

    pub(in crate::tui) fn scroll_down(&mut self, amount: usize) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(amount);
    }

    pub(in crate::tui) fn scroll_to_top(&mut self) {
        self.scroll_from_bottom = self.max_scroll;
    }

    pub(in crate::tui) fn scroll_to_bottom(&mut self) {
        self.scroll_from_bottom = 0;
    }
}
