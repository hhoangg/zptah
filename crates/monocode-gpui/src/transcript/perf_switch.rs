//! The transcript side of the conversation switch benchmark
//! (`crate::perf_switch`): the frame hook and the reading position.
use super::*;
use crate::perf_switch::Reading;

impl TranscriptView {
    /// Called from the trailing layout probe, after the list laid out.
    pub(super) fn perf_switch_frame(&self, cx: &Context<Self>) {
        crate::perf_switch::frame(
            cx.entity_id(),
            !self.messages.is_empty(),
            self.rows.len(),
            || self.perf_reading(),
        );
    }
    /// The row at the top of the list, as the reader sees it.
    pub(crate) fn perf_reading(&self) -> Option<Reading> {
        let top = self.list.logical_scroll_top();
        let key = self.rows.get(top.item_ix)?.key;
        let source = self.source.get(key.0)?;
        Some(Reading::new(
            &source.role,
            &source.text,
            key.1,
            f32::from(top.offset_in_item),
        ))
    }
    /// Scrolls to the prompt `turns` from the end, then a little further.
    pub(crate) fn perf_scroll_into_history(
        &mut self,
        turns: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(prompt) = self.prompt_from_end(turns) else {
            return false;
        };
        self.jump_to_prompt(prompt, cx);
        self.list.scroll_by(px(40.));
        true
    }
}
