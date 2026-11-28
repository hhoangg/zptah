//! The Runs mode of a workflow's page (`WorkflowRunPage.tsx`): the list of its runs, the picked run
//! drawn node by node on the graph canvas (read-only), and what the picked node did in the docked
//! inspector (`run_details.rs`).
//!
//! The run comes from the runner while it is live and from the runs file otherwise (read off the
//! UI thread by `refresh_workflows`); `run_page_sync` takes whichever is fresher whenever either
//! may have changed. The scene is rebuilt only when the run, the view or a pick changes, never per
//! frame.
use super::super::scheduler::{duration, when};
use super::super::*;
use super::actions;
use super::graph::{GraphCanvas, GraphEvent};
use super::page::run_status;
use super::run_model::*;
use super::view::alert;
use gpui::Div;
use monocode_core::mono::workflows::{
    cli::WorkflowHost,
    model::{CallerKind, RunStatus, WorkflowRun},
    queue::{queue_position, queued_count},
    run_graph_model::{
        Pick, Picks, RunView, VIEW_KEY, chosen_view, default_run_view, definition_changed,
        has_for_each,
    },
};
use monocode_core::store::Store;
use monocode_core::time::now_ms;

/// This app's own setting for the view of a workflow's runs (the React key is the default).
fn view_setting(workflow: &str) -> String {
    format!("native-workflow-run-view:{workflow}")
}

fn read_view(store: &Store, workflow: &str) -> Option<RunView> {
    let native = store.setting(&view_setting(workflow)).ok().flatten();
    let desktop = store
        .desktop_preferences()
        .and_then(|prefs| prefs.get(&format!("{VIEW_KEY}{workflow}")))
        .map(str::to_string);
    chosen_view(native.as_deref(), desktop.as_deref())
}

/// What the run page keeps while it is open.
#[derive(Default)]
pub(super) struct RunPage {
    /// The `(workflow, run)` the state belongs to.
    pub(super) key: Option<(String, String)>,
    pub(super) run: Option<WorkflowRun>,
    /// The runs file was read: a run that is not there is gone.
    pub(super) loaded: bool,
    /// The workflow was edited after the run took its copy.
    pub(super) changed: bool,
    /// The view the user chose last for this workflow, once read.
    pub(super) chosen: Option<RunView>,
    chosen_read: bool,
    pub(super) picks: Picks,
    /// The node the user picked.
    pub(super) picked: Option<String>,
    /// The node whose details show.
    pub(super) selection: Option<String>,
    pub(super) details: Option<Details>,
    /// Outputs shown whole, by `<node key>:<name>`.
    pub(super) show_all: HashSet<String>,
    pub(super) canvas: Option<Entity<GraphCanvas>>,
    _events: Option<Subscription>,
}

impl RunPage {
    /// The view being drawn: the chosen one, else the one that fits the run's size.
    pub(super) fn view(&self) -> Option<RunView> {
        let run = self.run.as_ref()?;
        Some(self.chosen.unwrap_or_else(|| default_run_view(run)))
    }
}

/// `mono` is the name of the Mono that started the run, when it is known.
pub(super) fn caller_text(run: &WorkflowRun, mono: Option<&str>) -> String {
    let caller = &run.caller;
    let node = run
        .definition
        .nodes
        .iter()
        .find(|n| n.id == caller.trigger_node)
        .map(|n| n.title.clone().unwrap_or_else(|| n.id.clone()))
        .filter(|title| !title.is_empty());
    let with_node = |kind: &str| match &node {
        Some(node) => format!("{kind} ({node})"),
        None => kind.to_string(),
    };
    match caller.kind {
        CallerKind::Manual => "Manual".into(),
        CallerKind::Schedule => with_node("Schedule"),
        CallerKind::Event => with_node("Event"),
        CallerKind::Mono => match mono.filter(|name| !name.is_empty()) {
            Some(name) => format!("Mono {name}"),
            None => "Mono".into(),
        },
    }
}

/// One run in the runs list: how it ended, when it started and how long it took.
fn run_row(
    run: &WorkflowRun,
    workflow: &str,
    picked: bool,
    place: Option<(usize, usize)>,
    color: u32,
    now: u64,
    cx: &mut Context<Monos>,
) -> AnyElement {
    let (label, dot) = run_status(run.status, color);
    let took = match run.ended_at {
        Some(ended) => duration(ended.saturating_sub(run.started_at)),
        None if run.status == RunStatus::Running => duration(now.saturating_sub(run.started_at)),
        None => "-".into(),
    };
    let (workflow, id) = (workflow.to_string(), run.id.clone());
    // A run that waits says its place; it has not started, so there is no time to show.
    let detail = match (run.status == RunStatus::Queued, place) {
        (true, Some((at, of))) => format!("Place {at} of {of}"),
        (true, None) => "Waiting".to_string(),
        (false, _) => format!("{} \u{b7} {took}", when(run.started_at, now)),
    };
    let queued = run.status == RunStatus::Queued;
    let (cancel_workflow, cancel_run) = (workflow.clone(), id.clone());
    div()
        .id(SharedString::from(format!("mono-workflow-run-{}", run.id)))
        .debug_selector({
            let id = run.id.clone();
            move || format!("mono-workflow-run-{id}")
        })
        .role(Role::Button)
        .aria_label(label)
        .aria_selected(picked)
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .px_2()
        .py_1p5()
        .rounded_md()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(12.))
        .cursor_pointer()
        .when(picked, |d| d.bg(content(0.08)))
        .when(!picked, |d| d.hover(|s| s.bg(content(0.05))))
        .child(if run.status == RunStatus::Running {
            super::view::running_dot(
                SharedString::from(format!("mono-workflow-pulse-{}", run.id)),
                color,
                8.,
            )
        } else {
            div()
                .size(px(8.))
                .flex_shrink_0()
                .rounded_full()
                .bg(dot)
                .into_any_element()
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().truncate().text_color(content(0.85)).child(label))
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .debug_selector({
                            let id = run.id.clone();
                            move || format!("wf-run-row-detail-{id}")
                        })
                        .child(detail),
                ),
        )
        .when(queued, |d| {
            let selector = format!("wf-run-row-cancel-{}", run.id);
            d.child(
                icon_button("wf-run-row-cancel", "Cancel queued run", "x")
                    .debug_selector(move || selector.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.run_cancel_queued(&cancel_workflow, &cancel_run, cx);
                    })),
            )
        })
        .on_click(cx.listener(move |this, _, _, cx| {
            this.open_workflow_route(WfRoute::Run(workflow.clone(), id.clone()), cx)
        }))
        .into_any_element()
}

fn meta(label: &'static str, value: impl IntoElement) -> Div {
    div()
        .flex()
        .h(px(24.))
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .child(div().text_color(content(0.45)).child(label))
        .child(
            div()
                .flex()
                .items_center()
                .text_color(content(0.85))
                .child(value),
        )
}

impl Monos {
    /// Opens the state for a run the first time its page is drawn.
    fn run_page_open(&mut self, scope: &Scope, workflow: &str, run: &str, cx: &mut Context<Self>) {
        let key = (workflow.to_string(), run.to_string());
        if self.wf.run.key.as_ref() == Some(&key) {
            return;
        }
        #[cfg(test)]
        {
            self.wf.run_opens += 1;
        }
        let canvas = cx.new(|_| GraphCanvas::new(scope.color));
        let events = cx.subscribe(&canvas, |this, _, event: &GraphEvent, cx| {
            this.run_graph_event(event, cx)
        });
        self.wf.run = RunPage {
            key: Some(key),
            canvas: Some(canvas),
            _events: Some(events),
            ..RunPage::default()
        };
        self.run_view_read(workflow, cx);
        self.run_page_sync(cx);
    }

    /// Reads the view chosen last for the workflow, off the UI thread.
    fn run_view_read(&mut self, workflow: &str, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let id = workflow.to_string();
        let read = id.clone();
        let task = cx
            .background_executor()
            .spawn(async move { locked(&store).ok().and_then(|db| read_view(&db, &read)) });
        cx.spawn(async move |this, cx| {
            let chosen = task.await;
            let _ = this.update(cx, |this, cx| {
                let page = &mut this.wf.run;
                if page.key.as_ref().map(|k| &k.0) != Some(&id) {
                    return;
                }
                page.chosen_read = true;
                // A choice made before the read came back is the newer one.
                if page.chosen.is_none() {
                    page.chosen = chosen;
                }
                this.run_page_rebuild(cx);
            });
        })
        .detach();
    }

    /// Takes the run from the runner while it has it, else from the runs file, and draws it
    /// again if it differs from the one drawn.
    pub(super) fn run_page_sync(&mut self, cx: &mut Context<Self>) {
        self.run_page_sync_at(now_ms(), cx)
    }

    /// `run_page_sync` as of `now`. A run that is going is drawn again on every look even when
    /// nothing in it changed, because its labels ("Running 3m") move with the clock.
    pub(super) fn run_page_sync_at(&mut self, now: u64, cx: &mut Context<Self>) {
        let Some((workflow, run_id)) = self.wf.run.key.clone() else {
            return;
        };
        let live = self.workflows.with_run_id(&run_id, |r| r.clone());
        let (next, loaded) = match (live, self.workflow_runs_of(&workflow)) {
            (Some(run), _) => (Some(run), true),
            (None, Some(runs)) => (runs.iter().find(|r| r.id == run_id).cloned(), true),
            (None, None) => (None, false),
        };
        let changed = match (&next, self.global_workflow(&workflow)) {
            // A run that waits has only a stub of the definition: it takes the live one at start.
            (Some(run), Some(current)) => {
                run.status != RunStatus::Queued && definition_changed(run, current)
            }
            _ => false,
        };
        let page = &mut self.wf.run;
        let same = page.run == next && page.loaded == loaded;
        page.changed = changed;
        if same {
            if page
                .run
                .as_ref()
                .is_some_and(|r| r.status == RunStatus::Running)
            {
                self.run_page_rebuild_at(now, cx);
            }
            return;
        }
        page.run = next;
        page.loaded = loaded;
        self.run_page_rebuild_at(now, cx);
        cx.notify();
    }

    /// Draws the run again: the scene, the picked node and its details.
    pub(super) fn run_page_rebuild(&mut self, cx: &mut Context<Self>) {
        self.run_page_rebuild_at(now_ms(), cx)
    }

    pub(super) fn run_page_rebuild_at(&mut self, now: u64, cx: &mut Context<Self>) {
        let page = &mut self.wf.run;
        let (Some(run), Some(view)) = (page.run.as_ref(), page.view()) else {
            page.selection = None;
            page.details = None;
            return;
        };
        page.selection = selection(run, page.picked.as_deref());
        page.details = page
            .selection
            .as_deref()
            .and_then(|key| details_of(run, key));
        // Until the remembered view is read the canvas waits, so it never flashes the other one.
        if !page.chosen_read {
            return;
        }
        let scene = run_scene(run, view, &page.picks, now);
        let selected = page.selection.clone();
        if let Some(canvas) = &page.canvas {
            canvas.update(cx, |canvas, cx| {
                canvas.set_scene(scene, cx);
                canvas.set_selected(selected, cx);
            });
        }
        cx.notify();
    }

    fn run_graph_event(&mut self, event: &GraphEvent, cx: &mut Context<Self>) {
        let page = &mut self.wf.run;
        match event {
            GraphEvent::Select(key) => page.picked = Some(key.clone()),
            GraphEvent::PickItem { group, index } => {
                page.picks.entry(group.clone()).or_default().item = Some(*index);
            }
            GraphEvent::PickRound { group, round } => {
                let pick: &mut Pick = page.picks.entry(group.clone()).or_default();
                pick.round = Some(*round);
                pick.item = None;
            }
            // The run page is read-only: the editing events never come.
            _ => return,
        }
        self.run_page_rebuild(cx);
    }

    /// Lanes side by side, or one item at a time; remembered for the workflow.
    pub(super) fn run_toggle_view(&mut self, cx: &mut Context<Self>) {
        let page = &mut self.wf.run;
        let (Some(view), Some((workflow, _))) = (page.view(), page.key.clone()) else {
            return;
        };
        let next = match view {
            RunView::Lanes => RunView::Single,
            RunView::Single => RunView::Lanes,
        };
        page.chosen = Some(next);
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = locked(&store)
                    .and_then(|db| db.save_setting(&view_setting(&workflow), next.name()));
            })
            .detach();
        self.run_page_rebuild(cx);
    }

    fn run_resume(&mut self, cx: &mut Context<Self>) {
        let Some((workflow, run)) = self.wf.run.key.clone() else {
            return;
        };
        self.workflow_said(None, true, cx);
        if self.run_gate_for_resume(&workflow, &run, true, cx) {
            return;
        }
        self.workflow_task(
            move |host| actions::resume_run(host, &workflow, &run),
            |this, notice, cx| this.workflow_said(notice, true, cx),
            cx,
        );
    }

    /// Opens a session the run used, as the app opens any session.
    pub(super) fn open_run_session(&mut self, session: &str, cx: &mut Context<Self>) {
        self.workflow_said(None, true, cx);
        match session.parse::<i64>() {
            Ok(id) => cx.emit(OpenWorkflowSession(id)),
            Err(_) => self.workflow_said(Some("Could not open this session.".into()), true, cx),
        }
    }

    /// The Runs mode: the runs list, the picked run on the canvas and its details beside it.
    pub(super) fn runs_mode(&mut self, scope: &Scope, cx: &mut Context<Self>) -> AnyElement {
        let Some(workflow) = self.wf.board.as_ref().map(|b| b.base.id.clone()) else {
            return div().into_any_element();
        };
        let picked = match &self.wf.route {
            Some(WfRoute::Run(w, run)) if *w == workflow => Some(run.clone()),
            _ => None,
        };
        if let Some(run) = &picked {
            self.run_page_open(scope, &workflow, run, cx);
        }
        let focus = self.wf.focus_canvas;
        let list = (!focus).then(|| self.runs_list(scope, &workflow, picked.as_deref(), cx));
        let center = self.run_center(scope, &workflow, picked.is_some(), cx);
        let inspector = (!focus).then(|| self.run_inspector(picked.is_some(), scope.color, cx));
        div()
            .id("mono-workflow-run-page")
            .debug_selector(|| "mono-workflow-run-page".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .children(list)
            .child(center)
            .children(inspector)
            .into_any_element()
    }

    /// The 240px list of the workflow's runs, newest first; the picked one is lit.
    fn runs_list(
        &mut self,
        scope: &Scope,
        workflow: &str,
        picked: Option<&str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let now = now_ms();
        let mut rows = div()
            .id("wf-runs-list-rows")
            .w_full()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .p_1()
            .flex()
            .flex_col()
            .gap_0p5();
        match self.workflow_runs_of(workflow).cloned() {
            Some(runs) if runs.is_empty() => {
                rows = rows.child(
                    div()
                        .px_2()
                        .py_3()
                        .text_size(px(12.))
                        .text_color(content(0.4))
                        .child("It hasn't run yet."),
                );
            }
            Some(runs) => {
                let of = queued_count(&runs);
                for run in &runs {
                    let place = queue_position(&runs, &run.id).map(|at| (at, of));
                    rows = rows.child(run_row(
                        run,
                        workflow,
                        picked == Some(run.id.as_str()),
                        place,
                        scope.color,
                        now,
                        cx,
                    ));
                }
            }
            None => {}
        }
        let slots = super::list::slots_subtitle(self.workflow_slots(workflow));
        let waiting = self
            .workflow_runs_of(workflow)
            .map_or(0, |runs| queued_count(runs));
        let bulk = workflow.to_string();
        div()
            .id("wf-runs-list")
            .debug_selector(|| "wf-runs-list".into())
            .w(px(super::layout::RUNS_W))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(content(0.1))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_shrink_0()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(content(0.4))
                    .child(div().child("Runs"))
                    .children(slots.map(|text| {
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::NORMAL)
                            .debug_selector(|| "wf-runs-slots".into())
                            .child(text)
                    }))
                    .when(waiting > 1, |d| {
                        d.child(
                            crate::modal::ghost_button(
                                "wf-runs-cancel-queued",
                                "Cancel all queued",
                            )
                            .debug_selector(|| "wf-runs-cancel-queued".into())
                            .h(px(22.))
                            .px_1p5()
                            .text_size(px(11.))
                            .cursor_pointer()
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.run_cancel_all_queued(&bulk, cx),
                            )),
                        )
                    }),
            )
            .child(rows)
            .into_any_element()
    }

    /// The middle of the Runs mode: how the run went, and the canvas it is drawn on. What sits
    /// over the canvas scrolls inside its own box, so it never pushes the canvas away.
    fn run_center(
        &mut self,
        scope: &Scope,
        workflow: &str,
        has_pick: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let frame = div().flex_1().min_w_0().min_h_0().flex().flex_col();
        let run = self.wf.run.run.clone();
        let Some(run) = run.filter(|_| has_pick) else {
            let text = if has_pick && self.wf.run.loaded {
                "This run is no longer kept."
            } else if has_pick {
                ""
            } else if self.workflow_runs_of(workflow).is_some_and(Vec::is_empty) {
                "It hasn't run yet."
            } else {
                "Pick a run to see how it went."
            };
            let gone = has_pick && self.wf.run.loaded;
            return frame
                .child(
                    div()
                        .id("wf-run-empty")
                        .debug_selector(move || {
                            if gone { "wf-run-gone" } else { "wf-run-empty" }.into()
                        })
                        .px_4()
                        .py_6()
                        .text_center()
                        .text_size(px(12.))
                        .text_color(content(0.45))
                        .child(text),
                )
                .into_any_element();
        };
        let lanes = has_for_each(&run).then(|| self.wf.run.view() == Some(RunView::Lanes));
        let (stopping, which, queued_workflow) =
            (workflow.to_string(), run.id.clone(), workflow.to_string());
        let queued_run = run.id.clone();
        let top = div()
            .id("wf-run-top")
            .debug_selector(|| "wf-run-top".into())
            .flex_shrink_0()
            .max_h(px(240.))
            .overflow_y_scroll()
            .border_b_1()
            .border_color(content(0.1))
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.run_meta(&run, scope.color, cx)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .pr_2()
                            .flex()
                            .items_center()
                            .gap_1()
                            .children(lanes.map(|lanes| {
                                icon_button(
                                    "wf-run-view",
                                    if lanes {
                                        "Show one item at a time"
                                    } else {
                                        "Show all items stacked"
                                    },
                                    if lanes { "layers" } else { "columns-3" },
                                )
                                .debug_selector(|| "wf-run-view".into())
                                .on_click(cx.listener(|this, _, _, cx| this.run_toggle_view(cx)))
                            }))
                            .children((run.status == RunStatus::Running).then(|| {
                                icon_button("wf-run-stop", "Stop run", "square")
                                    .debug_selector(|| "wf-run-stop".into())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.stop_workflow_run(&stopping, Some(&which));
                                        cx.notify();
                                    }))
                            }))
                            .children((run.status == RunStatus::Queued).then(|| {
                                icon_button("wf-run-cancel", "Cancel queued run", "x")
                                    .debug_selector(|| "wf-run-cancel".into())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.run_cancel_queued(&queued_workflow, &queued_run, cx);
                                    }))
                            }))
                            .children((run.status == RunStatus::Interrupted).then(|| {
                                icon_button("wf-run-resume", "Resume", "play")
                                    .debug_selector(|| "wf-run-resume".into())
                                    .on_click(cx.listener(|this, _, _, cx| this.run_resume(cx)))
                            })),
                    ),
            )
            .children(self.run_summary(&run))
            .when(self.wf.run.changed, |d| {
                d.child(
                    div()
                        .debug_selector(|| "wf-run-changed".into())
                        .flex_shrink_0()
                        .px_4()
                        .pb_3()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(content(0.45))
                        .child("The workflow changed since this run started."),
                )
            })
            .children(run.error.as_ref().map(|error| {
                alert(error.clone())
                    .id("wf-run-error")
                    .debug_selector(|| "wf-run-error".into())
                    .flex_shrink_0()
                    .px_4()
                    .pt_0()
                    .pb_3()
            }))
            // An interrupted run that is never resumed would otherwise forget the cloud
            // instances it left running. Only named here: nothing is stopped or deleted.
            .children(run.left_instances_warning().map(|warning| {
                alert(warning)
                    .id("wf-run-instances")
                    .debug_selector(|| "wf-run-instances".into())
                    .flex_shrink_0()
                    .px_4()
                    .pt_0()
                    .pb_3()
            }));
        frame
            .child(top)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .children(self.wf.run.canvas.clone()),
            )
            .into_any_element()
    }

    /// Cancels a run that waits in the queue: it never starts.
    pub(super) fn run_cancel_queued(&mut self, workflow: &str, run: &str, cx: &mut Context<Self>) {
        let (workflow, run) = (workflow.to_string(), run.to_string());
        self.workflow_said(None, true, cx);
        self.workflow_task(
            move |host| host.cancel_queued(&workflow, &run),
            |this, result, cx| {
                this.workflow_said(result.err(), true, cx);
            },
            cx,
        );
    }

    /// Cancels every run of the workflow that waits in its queue.
    pub(super) fn run_cancel_all_queued(&mut self, workflow: &str, cx: &mut Context<Self>) {
        let workflow = workflow.to_string();
        self.workflow_said(None, true, cx);
        self.workflow_task(
            move |host| {
                let runs = host.runs(&workflow).unwrap_or_default();
                for run in monocode_core::mono::workflows::queue::queued_fifo(&runs) {
                    if let Err(error) = host.cancel_queued(&workflow, &run.id) {
                        return Some(error);
                    }
                }
                None
            },
            |this, notice, cx| this.workflow_said(notice, true, cx),
            cx,
        );
    }

    /// Status, when it started and how long it took.
    fn run_meta(&self, run: &WorkflowRun, color: u32, cx: &mut Context<Self>) -> Div {
        let (label, dot) = run_status(run.status, color);
        let now = now_ms();
        let took = match run.ended_at {
            Some(ended) => duration(ended.saturating_sub(run.started_at)),
            None if run.status == RunStatus::Running => {
                duration(now.saturating_sub(run.started_at))
            }
            None => "-".into(),
        };
        let marker = if run.status == RunStatus::Running {
            super::view::running_dot("wf-run-pulse", color, 8.)
        } else {
            div()
                .size(px(8.))
                .flex_shrink_0()
                .rounded_full()
                .bg(dot)
                .into_any_element()
        };
        div()
            .debug_selector(|| "wf-run-meta".into())
            .flex_shrink_0()
            .px_4()
            .py_2()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_4()
            .gap_y_0p5()
            .child(meta(
                "Status",
                div()
                    .debug_selector(|| "wf-run-status".into())
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(marker)
                    .child(label),
            ))
            .child(meta(
                "Started",
                div()
                    .debug_selector(|| "wf-run-started".into())
                    .child(when(run.started_at, now)),
            ))
            .child(meta(
                "Took",
                div().debug_selector(|| "wf-run-took".into()).child(took),
            ))
            .child(meta(
                "Started by",
                div()
                    .debug_selector(|| "wf-run-caller".into())
                    .child(self.caller_label(run)),
            ))
            .children(self.queue_meta(run))
            .children(self.worktree_meta(run, cx))
    }

    /// Where a queued run stands: its place among the runs that wait.
    fn queue_meta(&self, run: &WorkflowRun) -> Option<Div> {
        if run.status != RunStatus::Queued {
            return None;
        }
        let waiting = self.workflow_runs_of(&run.workflow_id);
        let place = waiting.and_then(|runs| queue_position(runs, &run.id));
        let of = waiting.map_or(0, |runs| queued_count(runs));
        Some(meta(
            "Queue",
            div()
                .debug_selector(|| "wf-run-queue".into())
                .child(match place {
                    Some(place) => format!("Place {place} of {of}"),
                    None => "Waiting".to_string(),
                }),
        ))
    }

    /// The run's worktree: its branch, and a button to remove it when nothing uses it.
    fn worktree_meta(&self, run: &WorkflowRun, cx: &mut Context<Self>) -> Option<Div> {
        let tree = run.worktree.as_ref()?;
        if tree.is_removed() {
            return Some(meta(
                "Worktree",
                div()
                    .id("wf-run-worktree")
                    .debug_selector(|| "wf-run-worktree".into())
                    .max_w(px(420.))
                    .truncate()
                    .child(format!("Worktree removed ({})", tree.branch)),
            ));
        }
        let remove = super::run_tree_dialog::can_remove(run);
        Some(meta(
            "Worktree",
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id("wf-run-worktree")
                        .debug_selector(|| "wf-run-worktree".into())
                        .max_w(px(420.))
                        .truncate()
                        .child(format!("{} ({})", tree.branch, tree.path)),
                )
                .when(remove, |d| {
                    d.child(
                        crate::modal::ghost_button("wf-run-remove-tree", "Remove worktree")
                            .debug_selector(|| "wf-run-remove-tree".into())
                            .on_click(cx.listener(|this, _, _, cx| this.run_tree_open(cx))),
                    )
                }),
        ))
    }

    /// Who started the run: Manual, Schedule (trigger node), Event or Mono <name>.
    fn caller_label(&self, run: &WorkflowRun) -> String {
        let mono = run
            .caller
            .mono_id
            .as_deref()
            .map(|id| self.roster.get(id).map_or(id, |mono| mono.name.as_str()));
        caller_text(run, mono)
    }

    /// The summary the run ended with: its title and its Markdown text.
    fn run_summary(&self, run: &WorkflowRun) -> Option<AnyElement> {
        let summary = run.summary.as_ref()?;
        let mut body = div()
            .debug_selector(|| "wf-run-summary-text".into())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .text_size(px(12.))
            .line_height(px(20.));
        for (ix, mut row) in crate::markdown_model::parse_full(&summary.text)
            .blocks
            .iter()
            .flat_map(|b| crate::markdown::rows(&b.block))
            .enumerate()
        {
            crate::markdown::resolve_file_links(&mut row, "");
            body = body.child(div().w_full().min_w_0().flex_shrink_0().child(
                crate::markdown::body(
                    &row,
                    format!("wf-run-summary-{}-{ix}", run.id).into(),
                    crate::markdown::Tone::Normal,
                    None,
                    std::time::Instant::now(),
                ),
            ));
        }
        Some(
            div()
                .id("wf-run-summary")
                .debug_selector(|| "wf-run-summary".into())
                .flex_shrink_0()
                // The text can be long: it scrolls here instead of pushing the canvas away.
                .max_h(px(140.))
                .overflow_y_scroll()
                .px_4()
                .pb_3()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .debug_selector(|| "wf-run-summary-title".into())
                        .text_size(px(12.))
                        .text_color(content(0.85))
                        .child(summary.title.clone()),
                )
                .child(body)
                .into_any_element(),
        )
    }
}
