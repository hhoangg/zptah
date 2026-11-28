use crate::settings::SettingsView;
use gpui::{AnyElement, Context, div, prelude::*};

impl SettingsView {
    /// Settings > Chief: the Chief's own page, drawn by the Monos manager's view of it.
    pub(super) fn chief_page(&mut self, _cx: &mut Context<Self>) -> Vec<AnyElement> {
        match &self.chief {
            Some(view) => vec![div().w_full().child(view.clone()).into_any_element()],
            None => vec![super::super::widgets::note(
                "The Chief is not available here.",
            )],
        }
    }
}
