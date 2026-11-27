//! Release inactive transcript payloads while the composer and provider live on.
use super::*;

/// Histories shorter than this many bytes are shown whole at once.
const PREVIEW_MIN_BYTES: usize = 512 * 1024;
#[cfg(not(test))]
fn preview_min_bytes() -> usize {
    PREVIEW_MIN_BYTES
}
/// In tests every history counts as long unless a test says otherwise.
#[cfg(test)]
fn preview_min_bytes() -> usize {
    TEST_MIN_BYTES.with(|bytes| bytes.get())
}
#[cfg(test)]
thread_local! {
    /// Held by a test: the next load waits for it after the store read, so
    /// the preview is up and the full history is not.
    static TEST_GATE: std::cell::RefCell<Option<async_channel::Receiver<()>>> =
        const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
thread_local! {
    static TEST_MIN_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
thread_local! {
    /// How many previews the store has sent to this thread's chats.
    static PREVIEWS_SENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many previews the store sent to this thread's chats, for tests elsewhere.
#[cfg(test)]
pub(crate) fn previews_sent() -> usize {
    PREVIEWS_SENT.with(|sent| sent.get())
}

/// The sending end of a history preview, with where it should reach. Empty
/// when there is nothing to preview.
#[derive(Default)]
pub(super) struct PreviewSender {
    sender: Option<async_channel::Sender<Vec<monocode_core::transcript::Row>>>,
    rows_after: Option<usize>,
    #[cfg(test)]
    gate: Option<async_channel::Receiver<()>>,
}
impl PreviewSender {
    /// In tests, holds the load until the test lets it finish.
    pub(super) async fn wait_for_gate(&self) {
        #[cfg(test)]
        if let Some(gate) = &self.gate {
            let _ = gate.recv().await;
        }
    }

    /// Runs `read` with the store's preview request, if there is a preview.
    pub(super) fn with<R>(
        &self,
        read: impl FnOnce(Option<monocode_core::store::PreviewRequest<'_>>) -> R,
    ) -> R {
        match &self.sender {
            Some(sender) => {
                let mut send = |rows| {
                    let _ = sender.try_send(rows);
                };
                read(Some(monocode_core::store::PreviewRequest {
                    min_bytes: preview_min_bytes(),
                    rows_after: self.rows_after,
                    send: &mut send,
                }))
            }
            None => read(None),
        }
    }
}

pub(super) type HistoryAction = Box<dyn FnOnce(&mut Conversation, &mut Context<Conversation>)>;

impl Conversation {
    pub(super) fn history_ready(&self) -> bool {
        self.loaded && !self.history_unloaded && !self.loading
    }
    /// Whether `settings` came from the store or the user. Before the first
    /// load they are only `Conversation::new`'s defaults (the current account
    /// selection, not the account the session ran on).
    pub(crate) fn settings_loaded(&self) -> bool {
        self.loaded
    }
    pub(crate) fn needs_history_restore(&self) -> bool {
        self.history_unloaded || self.loading || (!self.loaded && self.summary.has_messages)
    }
    pub(crate) fn defer_history_action(
        &mut self,
        action: impl FnOnce(&mut Conversation, &mut Context<Conversation>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.history_actions.push_back(Box::new(action));
        self.load(cx);
    }
    pub(super) fn run_history_actions(&mut self, cx: &mut Context<Self>) {
        self.flush_wake_send(cx);
        while self.history_ready() && !self.stopping && !self.editing_last_turn.pending() {
            let Some(action) = self.history_actions.pop_front() else {
                break;
            };
            action(self, cx);
        }
    }

    pub(super) fn preserve_new_composer(&self, cx: &gpui::App) -> bool {
        self.settings.mono.is_none()
            && (!self.input.read(cx).content.is_empty() || !self.attachments.is_empty())
    }
    pub(super) fn enqueue_restored_prompt(&mut self, prompt: String, cx: &mut Context<Self>) {
        // The accepted prompt predates this composer draft. Queue it without
        // taking any files the user attached while history was restoring.
        let attachments = std::mem::take(&mut self.attachments);
        self.attachments = std::mem::take(&mut self.settings.attachments);
        let mode = if self.settings.orchestrator {
            Some("orchestrator")
        } else if self.settings.plan {
            Some("plan")
        } else if self.settings.operator {
            Some("operator")
        } else {
            None
        };
        self.enqueue_follow_up(prompt, mode.map(str::to_owned), cx);
        self.attachments = attachments;
    }
    pub(crate) fn history_loaded(&self) -> bool {
        self.loaded && !self.history_unloaded
    }
    pub(crate) fn history_weight(&self, cx: &gpui::App) -> usize {
        self.transcript.read(cx).history_weight()
    }
    /// Starts showing the newest turns of a long local chat while its history
    /// loads: the store sends them once it has read the blob, and they are
    /// painted by a separate, display-only transcript. Nothing is previewed
    /// for a Mono, a remote chat, a chat with nothing saved, or a pending
    /// search hit (which asks for turns the preview does not have).
    pub(super) fn start_history_preview(&mut self, cx: &mut Context<Self>) -> PreviewSender {
        // A preview belongs to the load that asked for it.
        self.history_generation += 1;
        let generation = self.history_generation;
        #[cfg(test)]
        let gate = TEST_GATE.with(|gate| gate.borrow_mut().take());
        let no_preview = || PreviewSender {
            #[cfg(test)]
            gate: gate.clone(),
            ..Default::default()
        };
        if self.settings.mono.is_some()
            || self.is_remote()
            || !self.summary.has_messages
            || self.pending_search.is_some()
            || !self.transcript.read(cx).source_rows().is_empty()
        {
            return no_preview();
        }
        let (tx, rx) = async_channel::unbounded::<Vec<monocode_core::transcript::Row>>();
        cx.spawn(async move |this, cx| {
            while let Ok(rows) = rx.recv().await {
                // Markdown is parsed off the main thread.
                let len = rows.len();
                let prepared = cx
                    .background_executor()
                    .spawn(async move { crate::transcript::PreparedHistory::full(rows) })
                    .await;
                // Counted when the store sent one, shown or not: the full
                // history can win the race.
                #[cfg(test)]
                PREVIEWS_SENT.with(|shown| shown.set(shown.get() + 1));
                let _ = this.update(cx, |this, cx| {
                    this.show_history_preview(prepared, len, generation, cx)
                });
            }
        })
        .detach();
        PreviewSender {
            sender: Some(tx),
            rows_after: self.transcript.read(cx).saved_history_rows_after(),
            #[cfg(test)]
            gate,
        }
    }

    fn show_history_preview(
        &mut self,
        prepared: crate::transcript::PreparedHistory,
        len: usize,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        // The full history may already be in, or a newer load started.
        if generation != self.history_generation
            || !self.loading
            || self.history_preview.is_some()
            || self.pending_search.is_some()
            || !self.transcript.read(cx).source_rows().is_empty()
        {
            return;
        }
        let template = self.transcript.read(cx).loading_preview(len);
        let preview = cx.new(|cx| {
            let mut preview = template;
            preview.install_prepared_history(prepared, cx);
            preview
        });
        crate::perf_switch::watch(self.summary.id, preview.entity_id());
        self.history_preview = Some(preview);
        cx.notify();
    }

    /// Takes the preview down as the full history arrives. A reader who
    /// scrolled it keeps their place.
    pub(super) fn finish_history_preview(
        &mut self,
        history: Option<&crate::transcript::PreparedHistory>,
        cx: &mut Context<Self>,
    ) {
        let Some(preview) = self.history_preview.take() else {
            return;
        };
        if let Some(position) =
            history.and_then(|history| preview.read(cx).preview_position(history))
        {
            self.transcript
                .update(cx, |view, _| view.set_history_position(position));
        }
        cx.notify();
    }

    /// The source row the history should open around: a search hit waiting
    /// for the load, else where the reader left an evicted history.
    pub(super) fn history_preparation_anchor(&self, cx: &gpui::App) -> Option<usize> {
        // A Mono's search rows count from its first turn, not from its window.
        if self.settings.mono.is_some() {
            return None;
        }
        self.pending_search
            .as_ref()
            .map(|(row, _)| *row)
            .or_else(|| self.transcript.read(cx).saved_history_anchor())
    }
    /// Only a settled, saved conversation can lose its reconstructible payload.
    /// Keep drafts, queued messages and their edits in the live entity, and pin
    /// their history so row positions remain valid during background saves.
    pub(crate) fn unload_history(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.history_loaded()
            || self.loading
            || self.busy
            || self.stopping
            || self.editing_last_turn.pending()
            || self
                .btw
                .as_ref()
                .is_some_and(|sheet| sheet.read(cx).is_open() || sheet.read(cx).is_running())
            || self.status != "Ready"
            || self.settings.mono.is_some()
            || self.account_switching
            || self.preparing_worktree
            || self.planning_orchestration
            || self.orchestration_plan.is_some()
            || self.orchestration_progress.is_some()
            || self.settings.usage_limit.is_some()
            || !self.history_actions.is_empty()
            || self.mono_ui.external.is_some()
            || self.controls.is_some()
            || self.needs_approval()
            || self.attaching
            || !self.attachments.is_empty()
            || !self.input.read(cx).content.is_empty()
            || self.input.read(cx).is_composing()
            || self.follow_ups.state.revision != self.follow_ups.persisted
            || !self.follow_ups.state.messages.is_empty()
            || self.follow_ups.state.draft.is_some()
            || self.follow_ups.edit.is_some()
            || self.follow_ups.sending.is_some()
            || self.follow_ups.requested.is_some()
            || self.pending_search.is_some()
            || self.response_request.is_some()
            || self.response_menu.is_some()
        {
            return false;
        }
        let suspended = self
            .transcript
            .update(cx, |view, cx| view.suspend_history(cx));
        self.history_unloaded = suspended;
        if suspended && self.is_remote() {
            self.release_remote_snapshot();
        }
        suspended
    }
    /// Restore only the payload. A history cache miss must not replace live
    /// settings, queued state, composer text, attachments or provider controls.
    pub(super) fn reload_history(&mut self, cx: &mut Context<Self>) {
        if self.is_remote() {
            self.refresh_remote_snapshot(cx);
            return;
        }
        if self.loading || !self.history_unloaded {
            return;
        }
        self.loading = true;
        let id = self.summary.id;
        let store = self.store.clone();
        let mono = self.settings.mono.is_some();
        let anchor = self.history_preparation_anchor(cx);
        let preview = self.start_history_preview(cx);
        let work = cx.background_executor().spawn(async move {
            let probe = crate::perf_switch::clock();
            let (history, turns, before) = {
                let store = locked(&store)?;
                let opened =
                    preview.with(|request| store.load_for_open_previewing(id, false, request));
                (
                    opened.transcript?,
                    opened.settings?.unwrap_or_default().turns,
                    store.history_before(id)?,
                )
            };
            preview.wait_for_gate().await;
            let loaded = (
                crate::transcript::PreparedHistory::at(history.rows, mono, anchor),
                turns,
                before,
            );
            crate::perf_switch::job(id, probe);
            Ok::<_, String>(loaded)
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                this.finish_history_preview(
                    result.as_ref().ok().map(|(history, _, _)| history),
                    cx,
                );
                this.loading = false;
                match result {
                    Ok((history, turns, before)) => {
                        this.settings.turns = turns.clone();
                        let pager = this.history_pager();
                        let probe = crate::perf_switch::clock();
                        this.transcript.update(cx, |view, cx| {
                            view.set_turns(turns);
                            view.restore_prepared_history(history, cx);
                            view.set_history_pager(before, Some(pager));
                        });
                        crate::perf_switch::installed(
                            this.summary.id,
                            this.transcript.entity_id(),
                            probe,
                        );
                        this.history_unloaded = false;
                        this.sync_review(cx);
                        this.status = "Ready".into();
                        crate::perf_switch::ready(this.summary.id);
                        if let Some((row, query)) = this.pending_search.take() {
                            this.reveal_search(row, query, cx);
                        }
                        this.run_history_actions(cx);
                        this.pump_follow_ups(cx);
                        this.mono_flush_external(cx);
                    }
                    Err(error) => this.status = error,
                }
                cx.emit(SessionChanged);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use monocode_core::{store::Store, transcript::Row};
    use std::sync::Mutex;

    #[gpui::test]
    fn history_eviction_keeps_live_state_and_pins_drafts_running_and_failed_saves(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.status = "Ready".into();
            chat.settings.revision = 7;
            chat.settings.model = Some("chosen-model".into());
            chat.transcript.update(cx, |view, cx| {
                view.visible(false, cx);
                view.patch(
                    0,
                    vec![Row {
                        role: "assistant".into(),
                        text: "Saved answer".into(),
                        first: true,
                        sent_at: None,
                    }],
                    cx,
                );
            });
            chat.busy = true;
            assert!(!chat.unload_history(cx));
            chat.busy = false;
            chat.status = "Could not save history".into();
            assert!(!chat.unload_history(cx));
            chat.status = "Ready".into();
            chat.attachments.push("/tmp/unsent-image.png".into());
            assert!(!chat.unload_history(cx));
            chat.attachments.clear();
            chat.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
            assert!(!chat.unload_history(cx));
            assert_eq!(chat.input.read(cx).content, "Unsent draft");
            chat.input
                .update(cx, |input, cx| input.set_text(String::new(), cx));
            chat.follow_ups.state.revision = 2;
            chat.follow_ups.persisted = 1;
            assert!(!chat.unload_history(cx));
            chat.follow_ups.persisted = 2;
            assert!(chat.unload_history(cx));
            assert!(!chat.history_loaded());
            assert!(chat.loaded, "composer/settings lifecycle remains loaded");
            assert_eq!(chat.settings.revision, 7);
            assert_eq!(chat.settings.model.as_deref(), Some("chosen-model"));
            assert_eq!(chat.follow_ups.state.revision, 2);
            assert_eq!(chat.history_weight(cx), 0);
        });
    }

    #[gpui::test]
    fn hidden_mutations_wait_for_history_without_changing_settings_or_receipts(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.history_unloaded = true;
            // Keep the test's restore under its own control; no provider runs.
            chat.loading = true;
            chat.settings.revision = 17;
            chat.settings.mono_message_id = Some("delivery-receipt".into());
            chat.model = Some("original-model".into());
            chat.input
                .update(cx, |input, cx| input.set_text("Unsent draft".into(), cx));
            let transfer = Settings {
                revision: 99,
                model: Some("new-model".into()),
                ..Default::default()
            };
            chat.quick_submit("Queued API prompt".into(), transfer, cx);
            assert!(!chat.busy);
            assert_eq!(chat.settings.revision, 17);
            assert_eq!(chat.model.as_deref(), Some("original-model"));
            assert_eq!(
                chat.settings.mono_message_id.as_deref(),
                Some("delivery-receipt")
            );
            assert_eq!(chat.input.read(cx).content, "Unsent draft");
            assert_eq!(chat.history_actions.len(), 1);
            assert!(!chat.ready_for_external(cx));
            assert!(chat.mono_post("assistant", "Report".into(), cx).is_err());
            assert!(chat.transcript.read(cx).source_rows().is_empty());
            // Replace the launch intent with observable, provider-free work to
            // verify FIFO actions cannot run against the empty suspended view.
            chat.history_actions.clear();
            chat.defer_history_action(
                |chat, cx| {
                    assert_eq!(
                        chat.transcript.read(cx).source_rows()[0].text,
                        "Restored history"
                    );
                    chat.settings.revision += 1;
                },
                cx,
            );
            chat.defer_history_action(
                |chat, _| {
                    assert_eq!(chat.settings.revision, 18);
                    chat.settings.revision += 1;
                },
                cx,
            );
            chat.run_history_actions(cx);
            assert_eq!(chat.settings.revision, 17);
            assert_eq!(chat.history_actions.len(), 2);
            chat.transcript.update(cx, |view, cx| {
                view.restore_history(
                    vec![Row {
                        role: "user".into(),
                        text: "Restored history".into(),
                        first: true,
                        sent_at: None,
                    }],
                    cx,
                )
            });
            chat.history_unloaded = false;
            chat.loading = false;
            chat.run_history_actions(cx);
            assert_eq!(chat.settings.revision, 19);
            assert!(chat.history_actions.is_empty());
            assert_eq!(
                chat.settings.mono_message_id.as_deref(),
                Some("delivery-receipt")
            );
            chat.follow_ups.state.paused = true;
            chat.attachments.push("/tmp/new-composer-image.png".into());
            assert!(chat.preserve_new_composer(cx));
            chat.enqueue_restored_prompt("Accepted before restore".into(), cx);
            assert_eq!(chat.input.read(cx).content, "Unsent draft");
            assert_eq!(
                chat.attachments,
                vec![std::path::PathBuf::from("/tmp/new-composer-image.png")]
            );
            assert_eq!(
                chat.follow_ups.state.messages[0].text,
                "Accepted before restore"
            );
            assert!(chat.follow_ups.state.messages[0].attachments.is_empty());
        });
    }

    #[gpui::test]
    fn restored_external_prompt_keeps_its_files_separate_from_new_composer_files(
        cx: &mut TestAppContext,
    ) {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let session = store.create("codex", "/project").unwrap();
        let chat = cx.new(|cx| Conversation::new(session, Arc::new(Mutex::new(store)), cx));
        chat.update(cx, |chat, cx| {
            chat.loaded = true;
            chat.follow_ups.state.paused = true;
            chat.input
                .update(cx, |input, cx| input.set_text("New draft".into(), cx));
            chat.attachments.push("/tmp/new-draft.png".into());
            chat.settings
                .attachments
                .push("/tmp/accepted-prompt.png".into());
            chat.enqueue_restored_prompt("Accepted prompt".into(), cx);
            assert_eq!(chat.input.read(cx).content, "New draft");
            assert_eq!(
                chat.attachments,
                vec![std::path::PathBuf::from("/tmp/new-draft.png")]
            );
            assert!(chat.settings.attachments.is_empty());
            let queued = &chat.follow_ups.state.messages[0];
            assert_eq!(queued.text, "Accepted prompt");
            assert_eq!(
                queued.attachments,
                vec![std::path::PathBuf::from("/tmp/accepted-prompt.png")]
            );
            assert!(!chat.busy);
        });
    }

    /// A saved local chat of `turns` turns; its history is not loaded yet.
    fn unloaded_chat(
        turns: usize,
        cx: &mut TestAppContext,
    ) -> (Vec<Row>, gpui::Entity<Conversation>) {
        let (rows, summary, database) = saved_chat_parts(turns);
        let chat = cx.new(|cx| Conversation::new(summary, database, cx));
        (rows, chat)
    }

    fn saved_chat_parts(
        turns: usize,
    ) -> (
        Vec<Row>,
        monocode_core::store::Session,
        crate::runtime::Database,
    ) {
        let mut store = Store::open_monocode(Path::new(":memory:")).unwrap();
        let session = store.create("claude", "/project").unwrap();
        let rows: Vec<Row> = (0..turns)
            .flat_map(|turn| {
                ["user", "assistant"].map(|role| Row {
                    role: role.into(),
                    text: format!("{role} {turn}"),
                    first: true,
                    sent_at: None,
                })
            })
            .collect();
        store.write_rows(session.id, 0, &rows).unwrap();
        let summary = store.sessions().unwrap().pop().unwrap();
        (rows, summary, Arc::new(Mutex::new(store)))
    }

    #[gpui::test]
    fn a_history_preview_never_receives_actions_and_gives_way_to_the_full_history(
        cx: &mut TestAppContext,
    ) {
        let (rows, chat) = unloaded_chat(60, cx);
        let ran = std::rc::Rc::new(std::cell::Cell::new(0));
        let transcript = chat.update(cx, |chat, cx| {
            chat.load(cx);
            assert!(chat.loading);
            // What the store would send as soon as it has read the history.
            chat.show_history_preview(
                crate::transcript::PreparedHistory::full(rows[80..].to_vec()),
                rows.len() - 80,
                chat.history_generation,
                cx,
            );
            let preview = chat.history_preview.clone().expect("a preview");
            assert_ne!(preview.entity_id(), chat.transcript.entity_id());
            assert_eq!(preview.read(cx).source_rows(), &rows[80..]);
            assert!(
                chat.transcript.read(cx).source_rows().is_empty(),
                "the chat's own transcript still has nothing"
            );
            // An action waits for the full history, as it does without a preview.
            let counter = ran.clone();
            chat.defer_history_action(move |_, _| counter.set(counter.get() + 1), cx);
            assert_eq!(ran.get(), 0);
            assert!(!chat.history_ready());
            chat.transcript.clone()
        });
        cx.run_until_parked();
        chat.read_with(cx, |chat, cx| {
            assert!(chat.history_preview.is_none(), "the preview is gone");
            assert!(chat.history_loaded());
            assert_eq!(transcript.read(cx).source_rows(), rows);
        });
        assert_eq!(ran.get(), 1, "the action ran once, on the full history");
    }

    #[gpui::test]
    fn a_late_history_preview_is_ignored(cx: &mut TestAppContext) {
        let (rows, chat) = unloaded_chat(60, cx);
        chat.update(cx, |chat, cx| chat.load(cx));
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            chat.show_history_preview(
                crate::transcript::PreparedHistory::full(rows[80..].to_vec()),
                rows.len() - 80,
                chat.history_generation,
                cx,
            );
            assert!(chat.history_preview.is_none(), "the history is already in");
        });
    }

    /// Opens a saved chat in a window, with the full history held back after
    /// the store has sent its preview. Returns once the preview is up.
    fn open_with_the_history_held_back(
        turns: usize,
        cx: &mut TestAppContext,
    ) -> (
        Vec<Row>,
        gpui::Entity<Conversation>,
        async_channel::Sender<()>,
        &mut gpui::VisualTestContext,
    ) {
        let (rows, summary, database) = saved_chat_parts(turns);
        let (gate, held) = async_channel::bounded(1);
        TEST_GATE.with(|slot| *slot.borrow_mut() = Some(held));
        let (chat, cx) = cx.add_window_view(|_, cx| Conversation::new(summary, database, cx));
        chat.update(cx, |chat, cx| chat.load(cx));
        cx.run_until_parked();
        (rows, chat, gate, cx)
    }

    #[gpui::test]
    fn opening_a_long_saved_chat_paints_a_preview_before_its_full_history_and_hands_over(
        cx: &mut TestAppContext,
    ) {
        let before = previews_sent();
        let (rows, chat, gate, cx) = open_with_the_history_held_back(60, cx);
        assert_eq!(previews_sent(), before + 1, "the store sent it");
        // While the full history is held back.
        let (preview, first) = chat.read_with(cx, |chat, cx| {
            assert!(chat.loading && !chat.history_loaded());
            assert!(
                chat.transcript.read(cx).source_rows().is_empty(),
                "the chat's own transcript has nothing yet"
            );
            let preview = chat.history_preview.clone().expect("the preview is up");
            let shown = preview.read(cx);
            let source = shown.source_rows();
            assert!(!source.is_empty() && source.len() < rows.len());
            assert_eq!(source, &rows[rows.len() - source.len()..]);
            assert!(shown.rendered > 0, "the preview was painted");
            (preview.clone(), rows.len() - source.len())
        });
        // The reader scrolls the preview to one of its rows.
        let row = 6;
        preview.update(cx, |view, _| view.scroll_preview_to(row, gpui::px(5.)));
        cx.run_until_parked();
        gate.try_send(()).unwrap();
        cx.run_until_parked();
        chat.read_with(cx, |chat, cx| {
            assert!(chat.history_preview.is_none(), "the preview gave way");
            assert!(chat.history_loaded());
            let transcript = chat.transcript.read(cx);
            assert_eq!(transcript.source_rows(), rows);
            assert_eq!(
                transcript.top_row(),
                (Some(first + row), false),
                "the reader stays on the row the preview showed"
            );
        });
    }

    #[gpui::test]
    fn a_preview_that_no_longer_matches_the_history_hands_nothing_over(cx: &mut TestAppContext) {
        let (rows, chat, gate, cx) = open_with_the_history_held_back(60, cx);
        // The stored history is not what the preview was made from.
        chat.update(cx, |chat, cx| {
            chat.history_preview = None;
            let mut stale = rows[80..].to_vec();
            stale[6].text = "A message another window rewrote".into();
            let generation = chat.history_generation;
            chat.show_history_preview(
                crate::transcript::PreparedHistory::full(stale),
                rows.len() - 80,
                generation,
                cx,
            );
            let preview = chat.history_preview.clone().expect("a preview");
            preview.update(cx, |view, _| view.scroll_preview_to(6, gpui::px(5.)));
        });
        gate.try_send(()).unwrap();
        cx.run_until_parked();
        chat.read_with(cx, |chat, cx| {
            assert!(chat.history_preview.is_none());
            let transcript = chat.transcript.read(cx);
            assert_eq!(transcript.source_rows(), rows);
            let (top, following) = transcript.top_row();
            assert!(following, "no position was carried over: {top:?}");
        });
    }

    #[gpui::test]
    fn a_preview_from_an_older_load_is_dropped(cx: &mut TestAppContext) {
        let (rows, chat, gate, cx) = open_with_the_history_held_back(60, cx);
        chat.update(cx, |chat, cx| {
            let older = chat.history_generation - 1;
            chat.history_preview = None;
            chat.show_history_preview(
                crate::transcript::PreparedHistory::full(rows[80..].to_vec()),
                rows.len() - 80,
                older,
                cx,
            );
            assert!(chat.history_preview.is_none(), "another load's preview");
            let current = chat.history_generation;
            chat.show_history_preview(
                crate::transcript::PreparedHistory::full(rows[80..].to_vec()),
                rows.len() - 80,
                current,
                cx,
            );
            assert!(chat.history_preview.is_some());
        });
        // Each load starts a new generation, evicted reloads included.
        gate.try_send(()).unwrap();
        cx.run_until_parked();
        chat.update(cx, |chat, cx| {
            let generation = chat.history_generation;
            chat.transcript
                .update(cx, |view, cx| view.visible(false, cx));
            assert!(chat.unload_history(cx));
            chat.load(cx);
            assert_eq!(chat.history_generation, generation + 1);
        });
    }

    #[gpui::test]
    fn a_store_preview_is_only_sent_for_a_long_history(cx: &mut TestAppContext) {
        let before = previews_sent();
        let (_, short) = unloaded_chat(5, cx);
        short.update(cx, |chat, cx| chat.load(cx));
        cx.run_until_parked();
        assert_eq!(previews_sent(), before, "a short history is shown whole");
    }

    #[gpui::test]
    fn a_history_under_the_byte_threshold_is_shown_whole_without_a_preview(
        cx: &mut TestAppContext,
    ) {
        let (rows, chat) = unloaded_chat(60, cx);
        TEST_MIN_BYTES.with(|bytes| bytes.set(PREVIEW_MIN_BYTES));
        let before = previews_sent();
        chat.update(cx, |chat, cx| chat.load(cx));
        cx.run_until_parked();
        TEST_MIN_BYTES.with(|bytes| bytes.set(0));
        assert_eq!(previews_sent(), before);
        chat.read_with(cx, |chat, cx| {
            assert_eq!(chat.transcript.read(cx).source_rows(), rows);
        });
    }
}
