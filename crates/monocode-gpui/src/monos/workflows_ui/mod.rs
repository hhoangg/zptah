//! The pages of the global Workflows view: the list, one workflow (a page with the modes Edit,
//! Runs and Settings: the board, a workflow's runs and a run, and its settings), its review before
//! it may run, and the import page. Port of `MonoWorkflows.tsx`, `WorkflowPage.tsx`,
//! `WorkflowReviewPage.tsx` and `ImportWorkflowPage.tsx`, now hosted by `app_page::WorkflowsPage`
//! and independent of any Mono.
//!
//! The global workflow files are the source of truth and there is no change notification, so the
//! UI reads them again whenever something may have changed: when the view or a page opens, after
//! every action, when the app's own `workflows.*` actions saved (`reload_files`), and while the
//! view is shown, on a short clock that also notices a run starting or ending in the runner. All
//! file access and model probing runs off the UI thread. Actions go through `actions`, which
//! follows the same rules as the `workflows.*` CLI actions.
pub(in crate::monos) mod actions;
mod app_page;
#[cfg(test)]
mod app_page_tests;
mod board;
#[cfg(test)]
mod board_canvas_tests;
#[cfg(test)]
mod board_tests;
mod board_ui;
mod graph;
mod import;
#[cfg(test)]
mod import_tests;
mod inspector;
mod inspector_parts;
#[cfg(test)]
mod inspector_tests;
mod inspector_variables;
#[cfg(test)]
mod inspector_variables_tests;
mod layout;
#[cfg(test)]
mod layout_tests;
mod list;
pub(in crate::monos) mod page;
#[cfg(test)]
mod page_tests;
mod requirements;
pub(in crate::monos) mod review;
#[cfg(test)]
mod review_tests;
mod run_details;
mod run_gate;
#[cfg(test)]
mod run_gate_tests;
mod run_model;
#[cfg(test)]
mod run_model_tests;
mod run_page;
#[cfg(test)]
mod run_tests;
mod run_tree_dialog;
#[cfg(test)]
mod run_tree_tests;
#[cfg(test)]
pub(in crate::monos) mod tests;
pub(in crate::monos) mod view;

pub(crate) use actions::ReviewIntent;
pub(crate) use app_page::WorkflowsPage;
pub(in crate::monos) use import::{apply_picked_file, pick_text_file, preview_later};

use super::*;
use crate::popover::{Align, MenuAction};
use monocode_core::mono::workflows::{
    cli::StoreHost,
    model::{Workflow, WorkflowRun},
    preflight::Preflight,
    store::{load_runs, load_workflows},
};
use std::collections::BTreeSet;

/// How often the tab looks again while it is shown.
const POLL: Duration = Duration::from_secs(2);
/// A look at the files with nothing running happens every this many polls (a schedule moves
/// `nextRunAt` along without any event).
const IDLE_POLLS: u32 = 15;

/// What a picker on a workflow page chooses for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Pick {
    /// An input of the file being imported, by name and declared type.
    Import(String, String),
    /// A drop-down of the board editor.
    Board(board::BoardPick),
    /// An input of the Run dialog.
    Gate(String),
    /// The Mono "Ask Mono to build a workflow" goes to.
    AskMono,
}

/// A page of the Workflows view (the list is no page: it is `None`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::monos) enum WfRoute {
    Workflow(String),
    Run(String, String),
    Board(String),
    Review(String, ReviewIntent),
    New,
    Import,
}

/// What the pages need from the app, taken fresh each time the view is drawn: the accent of the
/// drawings, the app's projects, and what a new session starts with.
#[derive(Clone, PartialEq)]
pub(in crate::monos) struct Scope {
    pub(in crate::monos) color: u32,
    /// The app's projects, as paths.
    pub(in crate::monos) projects: Vec<String>,
    /// The open project: where a model list is read from.
    pub(in crate::monos) project: String,
    /// The harness and model a new session starts with when a step sets none.
    pub(in crate::monos) default_kind: Option<monocode_core::provider::Kind>,
    pub(in crate::monos) default_model: Option<String>,
}

impl Default for Scope {
    fn default() -> Self {
        Self {
            color: crate::theme::ACCENT,
            projects: Vec::new(),
            project: String::new(),
            default_kind: None,
            default_model: None,
        }
    }
}

#[derive(Default)]
pub(super) struct WorkflowsUi {
    /// The workflows as last loaded, so the view shows them at once when it opens again.
    loaded: Option<Vec<Workflow>>,
    /// The runs of the workflows that need them (the open page), by workflow.
    runs: HashMap<String, Vec<WorkflowRun>>,
    /// How many runs of each workflow wait in its queue (only the workflows that have any).
    queued: HashMap<String, usize>,
    /// Whether the files were asked for already.
    requested: bool,
    /// Where the Ask Mono picker opens from.
    ask_anchor: AnchorSlot,
    /// The open page, or the list when `None`.
    route: Option<WfRoute>,
    /// Whether the Workflows view is the one on screen.
    open: bool,
    /// The page of a workflow is on its Settings mode (the Edit mode otherwise, for the routes
    /// that edit the board).
    settings: bool,
    /// The page hides its inner list and its inspector so the canvas has the room.
    focus_canvas: bool,
    /// Where the page's switcher menu and Run popover open from.
    run_anchor: AnchorSlot,
    /// What the pages need from the app (see `Scope`).
    scope: Scope,
    revision: u64,
    /// What a refused action says under the list, and under a page's header.
    notice: Option<String>,
    page_notice: Option<String>,
    poll: Option<gpui::Task<()>>,
    seen_running: Vec<String>,
    /// When the tab or a page was last drawn: the clock stops once nothing draws it.
    last_drawn: Option<std::time::Instant>,
    ticks: u32,
    prompts_closed: bool,
    review: Option<review::Review>,
    import: import::Import,
    /// The text an input field on the workflow page was last given from the stored value.
    seeds: HashMap<String, String>,
    focus: HashMap<String, gpui::FocusHandle>,
    pick: Option<Pick>,
    /// The open run page.
    run: run_page::RunPage,
    /// The open board editor.
    board: Option<board::BoardState>,
    /// The open "Remove worktree" confirmation of the run page.
    tree: Option<run_tree_dialog::TreeDialog>,
    /// The open Run or Resume dialog, and the number of the last check it asked for.
    gate: Option<run_gate::RunGate>,
    gate_generation: u64,
    /// What each workflow with `requires` found on this machine, by workflow id: the fingerprint
    /// it was checked for, when, and what it found.
    preflights: HashMap<String, (String, std::time::Instant, Preflight)>,
    /// The workflows being checked now.
    preflighting: HashSet<String>,
    /// How many times a run page built its state (a page sliding away must not add to it).
    #[cfg(test)]
    run_opens: u32,
}

impl Monos {
    /// The workflows, once loaded.
    pub(super) fn all_workflows(&self) -> Option<&Vec<Workflow>> {
        self.wf.loaded.as_ref()
    }

    pub(super) fn global_workflow(&self, id: &str) -> Option<&Workflow> {
        self.all_workflows()?.iter().find(|w| w.id == id)
    }

    pub(super) fn workflow_runs_of(&self, id: &str) -> Option<&Vec<WorkflowRun>> {
        self.wf.runs.get(id)
    }

    /// Whether the workflows were asked for already.
    pub(super) fn workflows_requested(&self) -> bool {
        self.wf.requested
    }

    /// How many workflows there are, once known.
    pub(super) fn workflow_count(&self) -> Option<usize> {
        self.all_workflows().map(Vec::len)
    }

    /// How the app judges workflows (model catalog and requirements): the `Workflows` entity's
    /// probes, so a test swaps them in one place.
    pub(crate) fn workflow_probes(&self, cx: &App) -> crate::workflows::Probes {
        self.workflows_entity
            .as_ref()
            .and_then(|workflows| workflows.upgrade())
            .map(|workflows| workflows.read(cx).probes())
            .unwrap_or_default()
    }

    /// The global workflow files, with how this app probes the model catalog and checks accounts.
    /// The user is the caller.
    pub(super) fn workflow_store(&self, cx: &App) -> StoreHost {
        match self.workflows_entity.as_ref().and_then(|w| w.upgrade()) {
            Some(workflows) => workflows.read(cx).store_host(cx),
            None => {
                let probes = crate::workflows::Probes::new();
                StoreHost::new(self.workflow_dir.clone(), self.workflow_signals.clone())
                    .with_probe(probes.probe())
                    .with_preflight(probes.preflight())
            }
        }
    }

    /// Whether the Workflows view is on screen.
    fn workflows_shown(&self) -> bool {
        self.wf.open
    }

    /// Called while the view is drawn: loads on first sight and keeps looking.
    pub(super) fn ensure_workflows(&mut self, cx: &mut Context<Self>) {
        self.wf.last_drawn = Some(std::time::Instant::now());
        // First sight, or the view shown again after the clock stopped: look at once.
        let first = !std::mem::replace(&mut self.wf.requested, true);
        if first || self.wf.poll.is_none() {
            self.refresh_workflows(cx);
        }
        if self.wf.poll.is_some() {
            return;
        }
        self.wf.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if this.update(cx, |this, cx| this.workflows_poll(cx)).ok() != Some(true) {
                    break;
                }
            }
        }));
    }

    /// One look of the clock: false once nothing shows workflows any more.
    fn workflows_poll(&mut self, cx: &mut Context<Self>) -> bool {
        // Not drawn for a beat or two (the view is hidden): stop reading files. Drawing it
        // again starts the clock and reads at once.
        let hidden = self
            .wf
            .last_drawn
            .is_some_and(|at| at.elapsed() > POLL * 2 + Duration::from_millis(500));
        if !self.workflows_shown() || hidden {
            if let Some(poll) = self.wf.poll.take() {
                poll.detach();
            }
            return false;
        }
        // A look asks for a draw, which is how a hidden view is told from a still one.
        cx.notify();
        let running: Vec<String> = self
            .workflows
            .running()
            .into_iter()
            .map(|(workflow, _)| workflow)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let changed = running != self.wf.seen_running;
        self.wf.seen_running = running;
        self.wf.ticks = self.wf.ticks.wrapping_add(1);
        let going = !self.wf.seen_running.is_empty();
        if changed || going || self.wf.ticks.is_multiple_of(IDLE_POLLS) {
            self.refresh_workflows(cx);
        }
        if changed || going {
            cx.notify();
        }
        true
    }

    /// Reads the workflow files again, off the UI thread.
    pub(super) fn refresh_workflows(&mut self, cx: &mut Context<Self>) {
        self.wf.revision += 1;
        let revision = self.wf.revision;
        let dir = self.workflow_dir.clone();
        // The open page also shows the workflow's recent runs.
        // A run the runner holds is shown from there, not from its file.
        let wanted = match &self.wf.route {
            Some(WfRoute::Workflow(id) | WfRoute::Board(id) | WfRoute::Run(id, _)) => {
                Some(id.clone())
            }
            _ => None,
        };
        let work = cx.background_executor().spawn(async move {
            let workflows = load_workflows(&dir)?.workflows;
            // The queue of each workflow that ran, for "N running / M queued".
            let queued: HashMap<String, usize> = workflows
                .iter()
                .filter(|w| w.last_run_id.is_some())
                .filter_map(|w| {
                    let n = monocode_core::mono::workflows::store::queued_count_of(&dir, &w.id)
                        .unwrap_or(0);
                    (n > 0).then(|| (w.id.clone(), n))
                })
                .collect();
            let runs = wanted
                .filter(|id| workflows.iter().any(|w| &w.id == id))
                .and_then(|id| load_runs(&dir, &id).ok().map(|runs| (id, runs)));
            Ok::<_, String>((workflows, runs, queued))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.wf.revision != revision {
                    return;
                }
                match result {
                    Ok((workflows, runs, queued)) => {
                        this.wf.queued = queued;
                        this.workflows_loaded(workflows, runs, cx)
                    }
                    Err(error) => eprintln!("Could not read the workflows: {error}"),
                }
            });
        })
        .detach();
    }

    fn workflows_loaded(
        &mut self,
        workflows: Vec<Workflow>,
        runs: Option<(String, Vec<WorkflowRun>)>,
        cx: &mut Context<Self>,
    ) {
        let count_changed = self.wf.loaded.as_ref().map(Vec::len) != Some(workflows.len());
        self.wf.runs.clear();
        if let Some((id, runs)) = runs {
            self.wf.runs.insert(id, runs);
        }
        self.wf.loaded = Some(workflows);
        // A workflow removed while open closes its pages.
        let gone = match &self.wf.route {
            Some(
                WfRoute::Workflow(id)
                | WfRoute::Run(id, _)
                | WfRoute::Board(id)
                | WfRoute::Review(id, _),
            ) => self.global_workflow(id).is_none(),
            _ => false,
        };
        if gone {
            self.wf.route = None;
            self.board_drop();
        }
        // The dialog of a workflow that is gone has nothing left to run.
        if self
            .wf
            .gate
            .as_ref()
            .is_some_and(|g| self.global_workflow(&g.workflow_id).is_none())
        {
            self.wf.gate = None;
        }
        // A review that is no longer on screen is dropped, not probed again.
        if !matches!(self.wf.route, Some(WfRoute::Review(..))) {
            self.wf.review = None;
        }
        self.review_sync(cx);
        if matches!(self.wf.route, Some(WfRoute::Run(..))) {
            self.run_page_sync(cx);
        }
        if self
            .wf
            .route
            .as_ref()
            .is_some_and(|r| board::is_board(r).is_some())
        {
            self.board_sync(cx);
        }
        self.refresh_preflights(cx);
        if count_changed {
            self.changed(cx);
        } else {
            cx.notify();
        }
    }

    /// Runs `work` over the workflow files off the UI thread, then `done` here, and reads the
    /// files again.
    pub(super) fn workflow_task<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut StoreHost) -> T + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let mut host = self.workflow_store(cx);
        let task = cx
            .background_executor()
            .spawn(async move { work(&mut host) });
        cx.spawn(async move |this, cx| {
            let out = task.await;
            let _ = this.update(cx, |this, cx| {
                done(this, out, cx);
                this.refresh_workflows(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows what a refused action says, or clears it when the action went through.
    pub(super) fn workflow_said(
        &mut self,
        notice: Option<String>,
        page: bool,
        cx: &mut Context<Self>,
    ) {
        if page {
            self.wf.page_notice = notice;
        } else {
            self.wf.notice = notice;
        }
        cx.notify();
    }

    /// The list is left: its notice goes (React resets it on unmount).
    pub(super) fn wf_leave_tab(&mut self) {
        self.wf.notice = None;
        self.wf.gate = None;
    }

    /// A page is closed: its notice and an open review go.
    pub(super) fn wf_leave_page(&mut self) {
        self.wf.page_notice = None;
        self.wf.gate = None;
        self.board_drop();
        if matches!(self.wf.route, Some(WfRoute::Review(..))) {
            self.wf.review = None;
        }
    }

    /// The Workflows view opens or closes (it is the one on screen, or not).
    pub(crate) fn workflows_view_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.wf.open == shown {
            return;
        }
        self.wf.open = shown;
        if shown {
            // Opened fresh: the list, read again.
            self.wf.route = None;
            self.refresh_workflows(cx);
        } else {
            self.wf_leave_page();
            self.wf_leave_tab();
            self.wf.route = None;
            self.picker = None;
        }
        cx.notify();
    }

    /// Opens a workflow page and reads its files again.
    pub(super) fn open_workflow_route(&mut self, route: WfRoute, cx: &mut Context<Self>) {
        self.wf.page_notice = None;
        // A board opens fresh from the files, and the board of another workflow is not kept.
        let key = board::is_board(&route);
        if matches!(route, WfRoute::Board(_) | WfRoute::New)
            || self.wf.board.as_ref().map(|b| &b.key) != key.as_ref()
        {
            self.board_drop();
        }
        // A run page opened from a link starts fresh.
        if matches!(route, WfRoute::Run(..)) {
            self.wf.run.key = None;
        }
        self.wf.settings = false;
        self.wf.route = Some(route);
        self.refresh_workflows(cx);
        cx.notify();
    }

    /// The `wf-` fields belong to the workflow pages; they go when a page is opened fresh.
    pub(super) fn clear_workflow_fields(&mut self) {
        self.fields.retain(|key, _| !key.starts_with("wf-"));
        self.wf.seeds.clear();
    }

    /// The Mono "Ask Mono to build a workflow" goes to unless the user picks another: the one
    /// open last, else the first.
    pub(super) fn ask_mono_default(&self) -> Option<String> {
        self.last_active
            .clone()
            .filter(|id| self.roster.get(id).is_some())
            .or_else(|| self.roster.monos.first().map(|m| m.id.clone()))
    }

    /// "Ask Mono to build a workflow": opens the Mono and starts a sentence in its composer for
    /// the user to finish. It never sends.
    pub(crate) fn ask_mono_for_workflow(
        &mut self,
        mono: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open(mono, window, cx);
        if self.active.as_deref() != Some(mono) {
            return;
        }
        let Some(chat) = self.chat() else {
            return;
        };
        let input = chat.read(cx).input.clone();
        if input.read(cx).is_composing() {
            return;
        }
        input.update(cx, |input, cx| {
            let draft = input.content.to_string();
            let next = import::append_prefill(&draft, ASK_MONO_WORKFLOW_DRAFT);
            if next != draft {
                input.set_text(next, cx);
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        cx.notify();
    }

    /// Opens the Mono picker of "Ask Mono to build a workflow".
    pub(super) fn open_ask_mono_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choices: Vec<(String, String)> = self
            .roster
            .monos
            .iter()
            .map(|mono| (mono.id.clone(), mono.label()))
            .collect();
        if choices.is_empty() {
            return;
        }
        let current = self.ask_mono_default().unwrap_or_default();
        let anchor = self.wf.ask_anchor.clone();
        self.open_workflow_picker(
            "Ask which Mono".into(),
            choices,
            current,
            Pick::AskMono,
            anchor,
            window,
            cx,
        );
    }

    fn workflow_focus(&mut self, key: &str, cx: &mut Context<Self>) -> gpui::FocusHandle {
        self.wf
            .focus
            .entry(key.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// A drop-down of the workflow pages (a project, on or off): the chosen value goes to
    /// `pick` when the menu answers.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn workflow_picker(
        &mut self,
        id: String,
        label: String,
        selected: String,
        choices: Vec<(String, String)>,
        current: String,
        pick: Pick,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let anchor = AnchorSlot::default();
        let focus = self.workflow_focus(&id, cx);
        let keyboard = (
            label.clone(),
            choices.clone(),
            current.clone(),
            pick.clone(),
            anchor.clone(),
        );
        let expanded = self.picker.is_some() && self.wf.pick.as_ref() == Some(&pick);
        let element_id: SharedString = id.clone().into();
        let selector = id.clone();
        let (open_label, open_choices, open_current, open_pick, open_anchor) =
            (label.clone(), choices, current, pick, anchor.clone());
        div()
            .id(element_id)
            .debug_selector(move || selector.clone())
            .role(Role::ComboBox)
            .aria_label(format!("{label}: {selected}"))
            .aria_expanded(expanded)
            .track_focus(&focus)
            .tab_index(0)
            .relative()
            .h(px(28.))
            .max_w_full()
            .flex_shrink_0()
            .px_2()
            .rounded_md()
            .bg(content(0.1))
            .hover(|s| s.bg(content(0.14)))
            .flex()
            .items_center()
            .gap_1()
            .text_size(px(12.))
            .line_height(px(16.))
            .child(anchor.probe())
            .child(div().min_w_0().truncate().child(selected))
            .child(icon("chevron-down", 12.))
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if matches!(
                        event.keystroke.key.as_str(),
                        "enter" | "space" | "down" | "up"
                    ) {
                        let (label, choices, current, pick, anchor) = keyboard.clone();
                        this.open_workflow_picker(
                            label, choices, current, pick, anchor, window, cx,
                        );
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_workflow_picker(
                    open_label.clone(),
                    open_choices.clone(),
                    open_current.clone(),
                    open_pick.clone(),
                    open_anchor.clone(),
                    window,
                    cx,
                );
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn open_workflow_picker(
        &mut self,
        label: String,
        choices: Vec<(String, String)>,
        current: String,
        pick: Pick,
        anchor: AnchorSlot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items = choices
            .into_iter()
            .map(|(value, text)| {
                MenuAction::new(value.clone(), text)
                    .checked(value == current)
                    .into()
            })
            .collect();
        let menu = cx.new(|cx| {
            Menu::new(label, items, window, cx)
                .width(240.)
                .select_value(&current)
        });
        let target = pick.clone();
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            if let MenuEvent::Picked(value, _) = event {
                this.workflow_picked(&target, value.to_string(), cx);
            }
            this.picker = None;
            this.wf.pick = None;
            cx.notify();
        });
        self.wf.pick = Some(pick);
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width: 240.,
            align: Align::Start,
            _events: vec![picked],
        });
        cx.notify();
    }

    fn workflow_picked(&mut self, pick: &Pick, value: String, cx: &mut Context<Self>) {
        match pick {
            Pick::Import(name, ty) => self.import_choose(name, ty, &value, cx),
            Pick::Board(pick) => self.board_picked(pick, &value, cx),
            Pick::Gate(name) => self.run_gate_picked(name, &value, cx),
            Pick::AskMono => cx.emit(AskMonoForWorkflow(value)),
        }
    }
}

/// What "Ask Mono to build a workflow" starts in the chat, for the user to finish.
pub(super) const ASK_MONO_WORKFLOW_DRAFT: &str = "Help me build a workflow that ";
