//! Two confirmations of the workflow pages, both on the shared modal: which run Stop ends when
//! several go at once, and what removing a workflow leaves behind (runs that wait, which are
//! cancelled, and worktrees that stay on disk).
use super::super::scheduler::{duration, when};
use super::super::*;
use gpui::Div;
use monocode_core::mono::workflows::{
    cli::{WorkflowHost, worktrees_left, worktrees_left_text},
    queue::queued_fifo,
};
use monocode_core::time::now_ms;

/// Which run to stop: the workflow has more than one going.
pub(super) struct StopAsk {
    pub(super) workflow: String,
    /// `(run id, label)`, oldest first.
    pub(super) runs: Vec<(String, String)>,
}

/// What removing a workflow takes or leaves, asked before it goes.
pub(super) struct RemoveAsk {
    pub(super) workflow: String,
    pub(super) name: String,
    pub(super) page: bool,
    /// The runs that wait: removing the workflow cancels them first.
    pub(super) queued: Vec<String>,
    /// `(path, branch)` of the worktrees that stay on disk.
    pub(super) trees: Vec<(String, String)>,
    /// Runs asked for that no claim answered yet: they go with the workflow.
    pub(super) requests: usize,
}

/// "Cancel all queued" asks first: how many runs it would end.
pub(super) struct CancelAsk {
    pub(super) workflow: String,
    pub(super) count: usize,
}

/// What a row of the stop dialog says about a run.
pub(super) fn run_label(number: Option<u64>, started_at: u64, now: u64) -> String {
    let name = number.map_or_else(|| "Run".to_string(), |n| format!("Run #{n}"));
    format!(
        "{name} \u{b7} started {} \u{b7} {}",
        when(started_at, now),
        duration(now.saturating_sub(started_at))
    )
}

/// The lines the removal dialog shows.
pub(super) fn remove_lines(ask: &RemoveAsk) -> Vec<String> {
    let mut lines = vec![format!(
        "Goes: the workflow with its steps, and its run history{}.",
        if ask.requests > 0 {
            format!(
                ", and {} run{} asked for that did not start yet",
                ask.requests,
                if ask.requests == 1 { "" } else { "s" }
            )
        } else {
            String::new()
        }
    )];
    lines.push("Stays: the sessions its runs opened, with their history.".to_string());
    if !ask.queued.is_empty() {
        lines.push(format!(
            "{} queued run{} will be cancelled; {} never start{}.",
            ask.queued.len(),
            if ask.queued.len() == 1 { "" } else { "s" },
            if ask.queued.len() == 1 { "it" } else { "they" },
            if ask.queued.len() == 1 { "s" } else { "" },
        ));
    }
    if !ask.trees.is_empty() {
        lines.push(worktrees_left_text(&ask.trees));
    }
    lines
}

fn dialog_text(text: impl Into<SharedString>, alpha: f32) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(17.))
        .text_color(content(alpha))
        .child(text.into())
}

impl Monos {
    /// Stop on a workflow: with one run going it stops, with several it asks which.
    pub(super) fn workflow_stop_clicked(&mut self, workflow: &str, cx: &mut Context<Self>) {
        let live = self.workflows.live_runs_of(workflow);
        if live.len() <= 1 {
            self.stop_workflow_run(workflow, None);
            cx.notify();
            return;
        }
        let now = now_ms();
        self.wf.stop_ask = Some(StopAsk {
            workflow: workflow.to_string(),
            runs: live
                .into_iter()
                .map(|(id, number, started)| (id, run_label(number, started, now)))
                .collect(),
        });
        cx.notify();
    }

    fn stop_ask_close(&mut self, cx: &mut Context<Self>) {
        self.wf.stop_ask = None;
        cx.notify();
    }

    /// Stops one run of the open dialog, or all of them, and closes it.
    fn stop_ask_stop(&mut self, run: Option<&str>, cx: &mut Context<Self>) {
        if let Some(ask) = self.wf.stop_ask.take() {
            self.stop_workflow_run(&ask.workflow, run);
        }
        cx.notify();
    }

    /// Remove on a workflow: removed at once when nothing needs saying, otherwise asked first.
    pub(in crate::monos) fn workflow_remove_clicked(
        &mut self,
        id: &str,
        page: bool,
        cx: &mut Context<Self>,
    ) {
        let (id, name, requests) = (
            id.to_string(),
            self.global_workflow(id)
                .map(|w| w.name.clone())
                .unwrap_or_default(),
            self.global_workflow(id).map_or(
                0,
                monocode_core::mono::workflows::model::Workflow::pending_requests,
            ),
        );
        let looked = id.clone();
        self.workflow_task(
            move |host| {
                let runs = host.runs(&looked).unwrap_or_default();
                let queued: Vec<String> = queued_fifo(&runs).iter().map(|r| r.id.clone()).collect();
                (queued, worktrees_left(&runs))
            },
            move |this, (queued, trees), cx| {
                // Always asked (D52): a removal cannot be taken back.
                this.wf.remove_ask = Some(RemoveAsk {
                    workflow: id,
                    name,
                    page,
                    queued,
                    trees,
                    requests,
                });
                cx.notify();
            },
            cx,
        );
    }

    fn remove_ask_close(&mut self, cx: &mut Context<Self>) {
        self.wf.remove_ask = None;
        cx.notify();
    }

    fn remove_ask_confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(ask) = self.wf.remove_ask.take() {
            self.workflow_remove_now(&ask.workflow, ask.page, !ask.queued.is_empty(), cx);
        }
        cx.notify();
    }

    /// "Cancel all queued" in the runs list: asks first.
    pub(super) fn run_cancel_all_clicked(
        &mut self,
        workflow: &str,
        count: usize,
        cx: &mut Context<Self>,
    ) {
        self.wf.cancel_ask = Some(CancelAsk {
            workflow: workflow.to_string(),
            count,
        });
        cx.notify();
    }

    fn cancel_ask_close(&mut self, cx: &mut Context<Self>) {
        self.wf.cancel_ask = None;
        cx.notify();
    }

    fn cancel_ask_confirm(&mut self, cx: &mut Context<Self>) {
        if let Some(ask) = self.wf.cancel_ask.take() {
            self.run_cancel_all_queued(&ask.workflow, cx);
        }
        cx.notify();
    }

    /// The cancel-all dialog, when it is open.
    pub(super) fn cancel_ask_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ask = self.wf.cancel_ask.as_ref()?;
        let count = ask.count;
        let weak = cx.weak_entity();
        let footer = crate::modal::actions()
            .child(
                crate::modal::ghost_button("wf-cancelall-cancel", "Keep them")
                    .debug_selector(|| "wf-cancelall-cancel".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_ask_close(cx))),
            )
            .child(
                crate::modal::danger_button("wf-cancelall-confirm", format!("Cancel {count} runs"))
                    .debug_selector(|| "wf-cancelall-confirm".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_ask_confirm(cx))),
            );
        let card = crate::modal::scroll_card(
            "wf-cancelall-scroll",
            crate::modal::max_card_height(window),
            crate::modal::title("Cancel every queued run?"),
            div().pt_3().child(dialog_text(
                format!("{count} runs wait for a free slot. Cancelled, they never start."),
                0.7,
            )),
            footer,
        )
        .w(px(420.))
        .id("wf-cancelall-ask")
        .debug_selector(|| "wf-cancelall-ask".into())
        .role(Role::Dialog)
        .aria_label("Cancel every queued run?");
        Some(
            crate::modal::window_overlay(
                window,
                crate::modal::overlay("wf-cancelall-modal", card, move |_, cx| {
                    let _ = weak.update(cx, |this, cx| this.cancel_ask_close(cx));
                }),
            )
            .into_any_element(),
        )
    }

    /// The stop dialog, when it is open.
    pub(super) fn stop_ask_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ask = self.wf.stop_ask.as_ref()?;
        let count = ask.runs.len();
        let mut rows = div().pt_3().flex().flex_col().gap_1();
        for (id, label) in &ask.runs {
            let (stop, selector) = (id.clone(), format!("wf-stop-run-{id}"));
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(content(0.8))
                            .child(label.clone()),
                    )
                    .child(
                        crate::modal::ghost_button(SharedString::from(selector.clone()), "Stop")
                            .debug_selector(move || selector.clone())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stop_ask_stop(Some(&stop), cx)
                            })),
                    ),
            );
        }
        let weak = cx.weak_entity();
        let footer = crate::modal::actions()
            .child(
                crate::modal::ghost_button("wf-stop-cancel", "Cancel")
                    .debug_selector(|| "wf-stop-cancel".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.stop_ask_close(cx))),
            )
            .child(
                crate::modal::danger_button("wf-stop-all", format!("Stop all {count}"))
                    .debug_selector(|| "wf-stop-all".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.stop_ask_stop(None, cx))),
            );
        let card = crate::modal::scroll_card(
            "wf-stop-scroll",
            crate::modal::max_card_height(window),
            crate::modal::title("Which run should stop?"),
            div()
                .child(dialog_text(
                    format!("{count} runs of this workflow are going. A stopped run ends within seconds; its branch and worktree stay."),
                    0.6,
                ))
                .child(rows),
            footer,
        )
        .w(px(420.))
        .id("wf-stop-ask")
        .debug_selector(|| "wf-stop-ask".into())
        .role(Role::Dialog)
        .aria_label("Which run should stop?");
        Some(
            crate::modal::window_overlay(
                window,
                crate::modal::overlay("wf-stop-modal", card, move |_, cx| {
                    let _ = weak.update(cx, |this, cx| this.stop_ask_close(cx));
                }),
            )
            .into_any_element(),
        )
    }

    /// The removal dialog, when it is open.
    pub(super) fn remove_ask_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let ask = self.wf.remove_ask.as_ref()?;
        let mut body = div().pt_3().flex().flex_col().gap_2();
        for line in remove_lines(ask) {
            body = body.child(dialog_text(line, 0.7));
        }
        let cancelling = !ask.queued.is_empty();
        let weak = cx.weak_entity();
        let footer = crate::modal::actions()
            .child(
                crate::modal::ghost_button("wf-remove-cancel", "Cancel")
                    .debug_selector(|| "wf-remove-cancel".into())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.remove_ask_close(cx))),
            )
            .child(
                crate::modal::danger_button(
                    "wf-remove-confirm",
                    if cancelling {
                        "Cancel queued runs and remove"
                    } else {
                        "Remove workflow"
                    },
                )
                .debug_selector(|| "wf-remove-confirm".into())
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| this.remove_ask_confirm(cx))),
            );
        let title = if ask.name.is_empty() {
            "Remove this workflow?".to_string()
        } else {
            format!("Remove \"{}\"?", ask.name)
        };
        let card = crate::modal::scroll_card(
            "wf-remove-scroll",
            crate::modal::max_card_height(window),
            crate::modal::title(title),
            body,
            footer,
        )
        .w(px(460.))
        .id("wf-remove-ask")
        .debug_selector(|| "wf-remove-ask".into())
        .role(Role::Dialog)
        .aria_label("Remove this workflow?");
        Some(
            crate::modal::window_overlay(
                window,
                crate::modal::overlay("wf-remove-modal", card, move |_, cx| {
                    let _ = weak.update(cx, |this, cx| this.remove_ask_close(cx));
                }),
            )
            .into_any_element(),
        )
    }
}
