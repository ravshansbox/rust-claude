use super::App;

#[derive(Clone, Copy)]
pub(in crate::tui) enum PickerKind {
    Session,
    Model,
    Effort,
}

pub(in crate::tui) struct Picker {
    pub(in crate::tui) kind: PickerKind,
    pub(in crate::tui) title: &'static str,
    pub(in crate::tui) items: Vec<(String, String)>,
    pub(in crate::tui) selected: usize,
}

impl App {
    /// Shows the picker in place of the prompt history search, so keys go
    /// to the list on screen. The input stays as it was.
    pub(in crate::tui) fn open_picker(&mut self, picker: Picker) {
        self.history_search = None;
        self.picker = Some(picker);
    }
}
