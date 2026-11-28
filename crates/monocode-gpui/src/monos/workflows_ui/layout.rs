//! The page of one workflow: a header (breadcrumb with a workflow switcher, the Enabled toggle,
//! the Edit | Runs | Settings modes, Run, Save, Focus and Tidy), an inner list, the canvas, a
//! docked inspector and a Problems bar. The three modes share one board (`board.rs`), so a draft
//! survives a switch of mode; the Runs mode adds the state of the open run (`run_page.rs`).
use super::super::panels::OpenPicker;
use super::super::*;
use super::board_ui::type_glyph;
use super::list::workflow_trigger_label;
use super::requirements::missing_requirements_badge;
use super::view::{header_button, needs_review_badge};
use super::{Scope, WfRoute};
use crate::popover::{Align, MenuAction, MenuItem};
use gpui::{Div, FontWeight, Stateful};
use monocode_core::mono::workflows::{
    board_edit::{picker_unavailable, type_label},
    board_model::NodeType,
    model::{TRIGGERS_MAX, interrupted_instances, left_instances_text},
    schedule::schedule_triggers,
};
use monocode_core::time::now_ms;

/// The inner list in Edit mode: the node palette.
pub(super) const RAIL_W: f32 = 44.;
/// The inner list in Runs mode.
pub(super) const RUNS_W: f32 = 240.;
/// The docked inspector.
pub(super) const INSPECTOR_W: f32 = 360.;

/// What the page shows of a workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::monos) enum WfMode {
    /// The board: palette rail, canvas and the form of the picked node.
    Edit,
    /// Its runs, and the picked run drawn on the canvas.
    Runs,
    /// Its name, description, inputs and requirements, and how decisions and summaries work.
    Settings,
}

impl WfMode {
    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Runs => "runs",
            Self::Settings => "settings",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        [Self::Edit, Self::Runs, Self::Settings]
            .into_iter()
            .find(|mode| mode.id() == id)
    }
}

/// The stable id of the add button of a node type in the rail.
fn add_id(ty: NodeType) -> &'static str {
    match ty {
        NodeType::Schedule => "wf-add-schedule",
        NodeType::Manual => "wf-add-manual",
        NodeType::Session => "wf-add-session",
        NodeType::Command => "wf-add-command",
        NodeType::If => "wf-add-if",
        NodeType::Switch => "wf-add-switch",
        NodeType::ForEach => "wf-add-forEach",
    }
}

/// The tooltip of a rail button: what the node does, or why it cannot be added now.
pub(super) fn rail_tip(ty: NodeType, hint: &str, why: Option<&str>) -> String {
    format!("{}: {}", type_label(ty), why.unwrap_or(hint))
}

/// The compact Enabled switch of the header.
fn enabled_switch(on: bool) -> Stateful<Div> {
    div()
        .id("wf-enabled-toggle")
        .debug_selector(|| "wf-enabled-toggle".into())
        .cursor_pointer()
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1p5()
        .px_1p5()
        .h(px(24.))
        .rounded_md()
        .hover(|s| s.bg(content(0.05)))
        .text_size(px(12.))
        .text_color(content(if on { 0.85 } else { 0.5 }))
        .child(super::super::panels::switch_track(
            "wf-enabled-track",
            "Enabled",
            on,
        ))
        .child("Enabled")
}

impl Monos {
    /// The mode the open page is in.
    pub(super) fn wf_mode(&self) -> WfMode {
        match &self.wf.route {
            Some(WfRoute::Workflow(_) | WfRoute::Run(..)) => WfMode::Runs,
            _ if self.wf.settings => WfMode::Settings,
            _ => WfMode::Edit,
        }
    }

    /// Switches the mode of the open page. The board (and its unsaved edits) is kept.
    pub(super) fn wf_set_mode(&mut self, mode: WfMode, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_mut() else {
            return;
        };
        // A workflow that was new and is saved now is a stored one like any other.
        if !b.is_new && b.key == "new" {
            b.key = b.base.id.clone();
        }
        let (id, is_new) = (b.base.id.clone(), b.is_new);
        let route = match mode {
            // Nothing has run before the first save.
            WfMode::Runs if is_new => return,
            WfMode::Runs => match &self.wf.run.key {
                Some((workflow, run)) if *workflow == id => WfRoute::Run(id, run.clone()),
                _ => WfRoute::Workflow(id),
            },
            _ if is_new => WfRoute::New,
            _ => WfRoute::Board(id),
        };
        self.wf.settings = mode == WfMode::Settings;
        self.wf.route = Some(route);
        self.refresh_workflows(cx);
        cx.notify();
    }

    /// Goes to another workflow in the mode that fits (its runs, or its board), asking first
    /// when edits are unsaved.
    fn wf_goto_workflow(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.wf.board.as_ref().is_some_and(|b| b.base.id == id) {
            return;
        }
        let to = match self.wf_mode() {
            WfMode::Runs => WfRoute::Workflow(id.to_string()),
            _ => WfRoute::Board(id.to_string()),
        };
        self.board_request_leave(Some(to), cx);
    }

    // ---- menus --------------------------------------------------------------------------------

    fn wf_open_switcher(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let current = b.base.id.clone();
        let anchor = b.switch_anchor.clone();
        let items: Vec<MenuItem> = self
            .all_workflows()
            .map(|all| {
                all.iter()
                    .map(|w| {
                        MenuAction::new(w.id.clone(), w.name.clone())
                            .icon("zap")
                            .checked(w.id == current)
                            .into()
                    })
                    .collect()
            })
            .unwrap_or_default();
        if items.is_empty() {
            return;
        }
        self.open_board_menu(
            "Switch workflow",
            items,
            anchor,
            260.,
            Align::Start,
            window,
            cx,
        );
    }

    pub(super) fn board_open_more_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.wf.board.as_ref() else {
            return;
        };
        let (anchor, is_new) = (b.more_anchor.clone(), b.is_new);
        let items = vec![
            MenuAction::new("export", "Export").icon("share").into(),
            MenuAction::new("remove", "Remove")
                .icon("trash")
                .danger()
                .disabled(is_new)
                .into(),
        ];
        self.open_board_menu("Workflow menu", items, anchor, 200., Align::End, window, cx);
    }

    // ---- the header ---------------------------------------------------------------------------

    fn wf_header(&mut self, mode: WfMode, cx: &mut Context<Self>) -> AnyElement {
        let Some(b) = self.wf.board.as_ref() else {
            return div().into_any_element();
        };
        let (name, enabled, id) = (b.meta.name.clone(), b.meta.enabled, b.base.id.clone());
        let (is_new, can_save, saving) = (b.is_new, b.can_save(), b.saving);
        let changed = b.changed();
        let review = b.base.needs_review;
        let (switch_anchor, more_anchor) = (b.switch_anchor.clone(), b.more_anchor.clone());
        let running = !is_new && self.workflow_running(&id);
        let starting = !running
            && self
                .global_workflow(&id)
                .is_some_and(|w| !is_new && w.run_requested);
        let busy = running || starting;
        // "2 running / 1 queued" when several go or any waits.
        let slots = if is_new {
            None
        } else {
            super::list::slots_subtitle(self.workflow_slots(&id))
        };
        let focus = self.wf.focus_canvas;
        let run_anchor = self.wf.run_anchor.clone();
        let stop_id = id.clone();
        let run_id = id.clone();
        let show_name = if name.trim().is_empty() {
            "Untitled".to_string()
        } else {
            name
        };

        let breadcrumb = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("mono-page-back")
                    .debug_selector(|| "mono-page-back".into())
                    .role(Role::Button)
                    .aria_label("Workflows")
                    .cursor_pointer()
                    .flex_shrink_0()
                    .h(px(24.))
                    .px_1p5()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .text_size(px(13.))
                    .text_color(content(0.5))
                    .hover(|s| s.bg(content(0.05)).text_color(content(1.)))
                    .child("Workflows")
                    .on_click(cx.listener(|this, _, _, cx| this.board_request_back(cx))),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(13.))
                    .text_color(content(0.3))
                    .child("/"),
            )
            .child(
                div()
                    .id("wf-switcher")
                    .debug_selector(|| "wf-switcher".into())
                    .role(Role::Button)
                    .aria_label(format!("Workflow: {show_name}"))
                    .relative()
                    .cursor_pointer()
                    .min_w_0()
                    .h(px(24.))
                    .px_1p5()
                    .rounded_md()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .hover(|s| s.bg(content(0.05)))
                    .child(div().absolute().size_full().child(switch_anchor.probe()))
                    .child(div().min_w_0().truncate().child(show_name))
                    .child(icon("chevron-down", 12.).text_color(content(0.5)))
                    .on_click(cx.listener(|this, _, window, cx| this.wf_open_switcher(window, cx))),
            )
            .when(review, |d| d.child(needs_review_badge()));

        let modes = crate::segmented::segmented(
            "segmented",
            "wf-mode",
            "Workflow mode",
            mode.id(),
            &[("edit", "Edit"), ("runs", "Runs"), ("settings", "Settings")],
            cx,
            |this: &mut Self, id, cx| {
                if let Some(mode) = WfMode::from_id(id) {
                    this.wf_set_mode(mode, cx);
                }
            },
        );

        let run_hint = if busy {
            if running {
                "Already running"
            } else {
                "Starting"
            }
        } else {
            "Run now"
        };
        let run = div()
            .relative()
            .flex_shrink_0()
            .child(div().absolute().size_full().child(run_anchor.probe()))
            .child(
                header_button("mono-workflow-run", "Run", true, !busy)
                    .aria_label(run_hint)
                    .gap_1()
                    .child(icon("zap", 12.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if busy {
                            return;
                        }
                        if this
                            .wf
                            .board
                            .as_ref()
                            .is_some_and(|b| b.is_new || b.changed())
                        {
                            this.workflow_said(
                                Some("Save your changes first, then run it.".into()),
                                true,
                                cx,
                            );
                            return;
                        }
                        let review = this.wf.board.as_ref().is_some_and(|b| b.base.needs_review);
                        this.workflow_run_clicked(&run_id, review, true, cx);
                    })),
            );
        let editing = mode != WfMode::Runs || changed;

        div()
            .h(px(40.))
            .flex_shrink_0()
            .min_w_0()
            .overflow_hidden()
            .border_b_1()
            .border_color(content(0.1))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .child(breadcrumb)
            .child(
                enabled_switch(enabled)
                    .on_click(cx.listener(|this, _, _, cx| this.board_toggle_enabled(cx))),
            )
            .child(modes)
            .child(div().flex_1().min_w_0())
            .children(slots.map(|text| {
                div()
                    .flex_shrink_0()
                    .px_1p5()
                    .text_size(px(11.))
                    .text_color(content(0.45))
                    .debug_selector(|| "wf-slots".into())
                    .child(text)
            }))
            .when(running, |d| {
                d.child(
                    icon_button("mono-workflow-stop", "Stop", "square")
                        .debug_selector(|| "mono-workflow-stop".into())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.workflow_stop_clicked(&stop_id, cx)
                        })),
                )
            })
            .child(run)
            .when(editing, |d| {
                d.child(
                    header_button("wf-board-save", "Save", true, can_save && !saving).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if can_save {
                                this.board_save(false, false, false, cx);
                            }
                        }),
                    ),
                )
            })
            .when(mode == WfMode::Edit, |d| {
                d.child(
                    icon_button("wf-tidy", "Tidy the board", "wand")
                        .debug_selector(|| "wf-tidy".into())
                        .on_click(cx.listener(|this, _, _, cx| this.board_tidy(cx))),
                )
            })
            .when(mode != WfMode::Settings, |d| {
                d.child(
                    icon_button(
                        "wf-focus",
                        if focus {
                            "Show the list and the inspector"
                        } else {
                            "Focus the canvas"
                        },
                        if focus {
                            "window-restore"
                        } else {
                            "window-maximize"
                        },
                    )
                    .debug_selector(|| "wf-focus".into())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.wf.focus_canvas = !this.wf.focus_canvas;
                        cx.notify();
                    })),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(div().absolute().size_full().child(more_anchor.probe()))
                    .child(
                        icon_button("wf-board-more", "More", "more-horizontal")
                            .debug_selector(|| "wf-board-more".into())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.board_open_more_menu(window, cx)
                            })),
                    ),
            )
            .into_any_element()
    }

    /// What the workflow says about itself under the header: when it runs, whether it needs a
    /// review or has requirements missing, and the cloud instances an interrupted run left.
    fn wf_info_strip(&self) -> Option<AnyElement> {
        let b = self.wf.board.as_ref()?;
        let w = b.current();
        let stored = self.global_workflow(&b.base.id);
        let starting = stored.is_some_and(|s| s.run_requested) && !self.workflow_running(&w.id);
        let mut line = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_3()
            .gap_y_0p5()
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.5))
            .child(div().child(format!(
                "Runs: {}",
                if w.enabled {
                    workflow_trigger_label(&w)
                } else {
                    "Paused".to_string()
                }
            )));
        if starting {
            line = line.child(div().child("Next: Starting\u{2026}"));
        } else if w.enabled
            && !schedule_triggers(&w).is_empty()
            && let Some(next) = stored.and_then(|s| s.next_run_at)
        {
            line = line.child(div().child(format!(
                "Next: {}",
                super::super::scheduler::when(next, now_ms())
            )));
        }
        if stored.is_some_and(|s| self.missing_requirements(s).is_some()) {
            line = line.child(missing_requirements_badge());
        }
        let missing = stored.and_then(|s| self.missing_requirements_section(s));
        let left = self
            .wf
            .runs
            .get(&b.base.id)
            .map(|runs| interrupted_instances(runs));
        let left = left.filter(|l| !l.is_empty()).map(|l| {
            super::view::alert(left_instances_text(
                "The interrupted runs of this workflow",
                &l,
            ))
            .id("mono-workflow-instances")
            .debug_selector(|| "mono-workflow-instances".into())
            .px_0()
            .pt_0()
        });
        Some(
            div()
                .id("wf-info")
                .debug_selector(|| "wf-info".into())
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .max_h(px(160.))
                .overflow_y_scroll()
                .border_b_1()
                .border_color(content(0.1))
                .px_3()
                .py_1()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(line)
                .children(missing)
                .children(left)
                .into_any_element(),
        )
    }

    // ---- the page -----------------------------------------------------------------------------

    /// The page of a workflow in whichever mode it is in.
    pub(in crate::monos) fn workflow_shell(
        &mut self,
        scope: &Scope,
        route: &WfRoute,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.ensure_workflows(cx);
        // The page is sliding away after Back: nothing to rebuild for it.
        if self.wf.route.as_ref() != Some(route) && self.wf.board.is_none() {
            return div().size_full().into_any_element();
        }
        if !self.board_open(scope, route, cx) {
            return div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .child(self.workflow_header("Workflow", None, cx))
                .into_any_element();
        }
        let mode = self.wf_mode();
        let header = self.wf_header(mode, cx);
        let banners = self.board_banners(cx);
        let info = self.wf_info_strip();
        let body = match mode {
            WfMode::Edit => self.wf_edit_body(scope, window, cx),
            WfMode::Runs => self.runs_mode(scope, cx),
            WfMode::Settings => self.wf_settings_body(scope, window, cx),
        };
        // Runs mode shows the run, not the draft: its problems bar appears only once the draft has
        // changed, so a flagged edit is not hidden by switching to a run.
        let draft_changed = self.wf.board.as_ref().is_some_and(|b| b.changed());
        let problems = (mode != WfMode::Runs || draft_changed)
            .then(|| self.wf_problems_bar(cx))
            .flatten();
        let leaving = self.board_leave_dialog(window, cx);
        let gate = self.run_gate_dialog(scope, window, cx);
        let tree = self.run_tree_dialog(window, cx);
        let stopping = self.stop_ask_dialog(window, cx);
        let removing = self.remove_ask_dialog(window, cx);
        let cancelling = self.cancel_ask_dialog(window, cx);
        div()
            .id("wf-board")
            .debug_selector(|| "wf-board".into())
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Cmd+S (Ctrl+S elsewhere) saves.
                if event.keystroke.modifiers.secondary()
                    && event.keystroke.key == "s"
                    && !window.has_pending_keystrokes()
                    && this
                        .wf
                        .board
                        .as_ref()
                        .is_some_and(super::board::BoardState::can_save)
                {
                    this.board_save(false, false, false, cx);
                    cx.stop_propagation();
                }
            }))
            .child(header)
            .children(super::view::notice_line(
                "mono-workflow-notice",
                self.wf.page_notice.as_deref(),
            ))
            .children(banners)
            .children(info)
            .child(body)
            .children(problems)
            .children(leaving)
            .children(gate)
            .children(tree)
            .children(stopping)
            .children(removing)
            .children(cancelling)
            .into_any_element()
    }

    /// Edit mode: the palette rail, the canvas and the docked inspector.
    fn wf_edit_body(
        &mut self,
        scope: &Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(b) = self.wf.board.as_ref() else {
            return div().into_any_element();
        };
        let (canvas, area, view, ask) = (
            b.canvas.clone(),
            b.area.clone(),
            b.view.clone(),
            b.loop_ask.clone(),
        );
        let focus = self.wf.focus_canvas;
        let rail = (!focus).then(|| self.wf_rail(cx));
        let inspector = (!focus).then(|| self.board_inspector(scope, view, window, cx));
        let loop_card = ask.map(|ask| self.board_loop_card(&ask, window, cx));
        div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .children(rail)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(div().absolute().size_full().child(area.layout_probe()))
                    .child(div().absolute().inset_0().child(canvas))
                    .children(loop_card.map(|card| {
                        // A press outside the card closes it, as a popover does.
                        div()
                            .absolute()
                            .inset_0()
                            .child(
                                div()
                                    .id("wf-loop-scrim")
                                    .debug_selector(|| "wf-loop-scrim".into())
                                    .absolute()
                                    .inset_0()
                                    .occlude()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| this.board_cancel_loop(cx)),
                                    ),
                            )
                            .child(card)
                    })),
            )
            .children(inspector)
            .into_any_element()
    }

    /// The 44px rail of Edit mode: one button for each kind of node the board can add.
    fn wf_rail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(b) = self.wf.board.as_ref() else {
            return div().into_any_element();
        };
        let triggers = b.current().nodes.iter().filter(|n| n.is_trigger()).count();
        debug_assert!(triggers <= TRIGGERS_MAX);
        let mut rail = div()
            .id("wf-rail")
            .debug_selector(|| "wf-rail".into())
            .w(px(RAIL_W))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(content(0.1))
            .py_2()
            .flex()
            .flex_col()
            .items_center()
            .gap_1();
        for (group_at, group) in monocode_core::mono::workflows::board_edit::picker_groups()
            .iter()
            .enumerate()
        {
            if group_at > 0 {
                rail = rail.child(
                    div()
                        .w(px(20.))
                        .h(px(1.))
                        .my_1()
                        .bg(content(0.1))
                        .flex_shrink_0(),
                );
            }
            for (ty, hint) in group.items {
                let (ty, hint) = (*ty, *hint);
                let why = picker_unavailable(ty, triggers);
                let enabled = why.is_none();
                // The reason it cannot be added now, else what the node does.
                let tip = rail_tip(ty, hint, why);
                rail = rail.child(
                    icon_button_enabled(add_id(ty), type_label(ty), type_glyph(ty), enabled)
                        .debug_selector(move || add_id(ty).into())
                        .tooltip(move |_, cx| {
                            let tip = tip.clone();
                            cx.new(|_| crate::theme::Tip(tip.into())).into()
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if enabled {
                                this.board_add(ty, cx);
                            }
                        })),
                );
            }
        }
        rail.into_any_element()
    }

    /// Settings mode: the workflow's own fields in one scrolling column.
    fn wf_settings_body(
        &mut self,
        scope: &Scope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(w) = self.wf.board.as_ref().map(|b| b.current()) else {
            return div().into_any_element();
        };
        let body = self.board_settings(scope, &w, window, cx);
        div()
            .id("wf-settings-page")
            .debug_selector(|| "wf-settings-page".into())
            .w_full()
            .min_w_0()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(div().mx_auto().w_full().max_w(px(640.)).child(body))
            .into_any_element()
    }

    /// The bar at the bottom: how many problems keep the workflow from running, the first one,
    /// and the whole list when opened. A problem picked selects its node.
    fn wf_problems_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let b = self.wf.board.as_ref()?;
        if b.issues.is_empty() && b.warnings.is_empty() {
            return None;
        }
        // Problems keep the workflow from running; warnings are advice that never blocks (D51).
        let count = b.issues.len();
        let advice = b.warnings.len();
        let first = b
            .issues
            .first()
            .or(b.warnings.first())
            .map(|i| i.message.clone())
            .unwrap_or_default();
        let open = b.show_issues;
        let (saving, enabled) = (b.saving, b.meta.enabled);
        let blocked = count > 0;
        let rows: Vec<(usize, String, Option<String>, bool)> = b
            .issues
            .iter()
            .map(|i| (i, false))
            .chain(b.warnings.iter().map(|i| (i, true)))
            .enumerate()
            .map(|(at, (i, warning))| (at, i.message.clone(), i.node_id.clone(), warning))
            .collect();
        let headline = match (count, advice) {
            (0, advice) => format!(
                "{advice} {}",
                if advice == 1 { "warning" } else { "warnings" }
            ),
            (count, 0) => format!(
                "{count} {}",
                if count == 1 { "problem" } else { "problems" }
            ),
            (count, advice) => format!(
                "{count} {} \u{b7} {advice} {}",
                if count == 1 { "problem" } else { "problems" },
                if advice == 1 { "warning" } else { "warnings" }
            ),
        };
        let dot = if blocked {
            gpui::Hsla::from(rgb(0xef4444)).opacity(0.7)
        } else {
            gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.8)
        };
        let mut bar = div()
            .id("wf-board-issues")
            .debug_selector(|| "wf-board-issues".into())
            .w_full()
            .flex_shrink_0()
            .border_t_1()
            .border_color(content(0.1))
            .px_4()
            .py_1p5()
            .text_size(px(11.))
            .line_height(px(16.))
            .text_color(content(0.6))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("wf-board-issues-toggle")
                            .debug_selector(|| "wf-board-issues-toggle".into())
                            .role(Role::Button)
                            .aria_expanded(open)
                            .cursor_pointer()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().size(px(6.)).flex_shrink_0().rounded_full().bg(dot))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_color(content(0.8))
                                    .child(headline),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(content(0.45))
                                    .child(first),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(b) = this.wf.board.as_mut() {
                                    b.show_issues = !b.show_issues;
                                }
                                cx.notify();
                            })),
                    )
                    .when(enabled && blocked, |d| {
                        d.child(
                            super::board_ui::banner_button(
                                "wf-board-issues-paused",
                                "Save paused",
                                !saving,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.board_save(true, false, false, cx),
                            )),
                        )
                    }),
            );
        if open {
            bar = bar.child(
                div()
                    .id("wf-problem-list")
                    .debug_selector(|| "wf-problem-list".into())
                    .max_h(px(140.))
                    .overflow_y_scroll()
                    .pt_1p5()
                    .pb_0p5()
                    .flex()
                    .flex_col()
                    .children(rows.into_iter().map(|(at, message, node, warning)| {
                        let picked = node.clone();
                        let kind = if warning { "wf-warning" } else { "wf-problem" };
                        div()
                            .id(SharedString::from(format!("{kind}-{at}")))
                            .debug_selector(move || format!("{kind}-{at}"))
                            .role(Role::Button)
                            .aria_label(message.clone())
                            .cursor_pointer()
                            .rounded_sm()
                            .pl_3()
                            .pr_1()
                            .py_0p5()
                            .when(warning, |d| {
                                d.text_color(gpui::Hsla::from(rgb(0xfbbf24)).opacity(0.85))
                            })
                            .hover(|s| s.bg(content(0.06)).text_color(content(1.)))
                            .child(message)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.board_pick_problem(picked.clone(), cx)
                            }))
                    })),
            );
        }
        Some(bar.into_any_element())
    }

    /// Opens a menu of the page, answered by `board_menu_picked`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_board_menu(
        &mut self,
        label: &'static str,
        items: Vec<MenuItem>,
        anchor: AnchorSlot,
        width: f32,
        align: Align,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = cx.new(|cx| Menu::new(label, items, window, cx).width(width));
        let picked = cx.subscribe(&menu, move |this, _, event: &MenuEvent, cx| {
            this.picker = None;
            if let MenuEvent::Picked(id, _) = event {
                this.board_menu_picked(label, id.to_string(), cx);
            }
            cx.notify();
        });
        self.picker = Some(OpenPicker {
            menu,
            anchor: Anchor::Slot(anchor),
            width,
            align,
            _events: vec![picked],
        });
        cx.notify();
    }

    /// What a pick in one of the page's menus does.
    pub(super) fn board_menu_picked(&mut self, menu: &str, id: String, cx: &mut Context<Self>) {
        match (menu, id.as_str()) {
            ("Switch workflow", id) => self.wf_goto_workflow(id, cx),
            (_, "export") => {
                // What is saved, or the draft when it has edits that are not.
                let shared = self.wf.board.as_ref().map(|b| {
                    if b.is_new || b.changed() {
                        b.current()
                    } else {
                        b.base.clone()
                    }
                });
                if let Some(w) = shared {
                    self.workflow_export(&w, cx);
                }
            }
            (_, "remove") => {
                if let Some(id) = self
                    .wf
                    .board
                    .as_ref()
                    .filter(|b| !b.is_new)
                    .map(|b| b.base.id.clone())
                {
                    self.workflow_remove_clicked(&id, true, cx);
                }
            }
            _ => {}
        }
    }
}
