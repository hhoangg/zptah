//! The page of a workflow: its header, the three modes, the rail, the docked inspector, the
//! Problems bar, Focus and Tidy, the Settings mode and the Runs mode. The rules each of them
//! draws are in `board_edit_tests.rs`, `run_graph_model_tests.rs` and the sibling test files.
use super::board::{BoardField, BoardPick, InspectorView};
use super::board_tests::{do_with, event, on_board, open_board, pick, stored_first, ticket};
use super::board_tests::{rename, type_in};
use super::graph::{BarKind, GraphEvent};
use super::layout::{INSPECTOR_W, RAIL_W, RUNS_W, WfMode};
use super::run_model_tests::{each_run, each_workflow, linear, nr, run_of};
use super::tests::{A, build, listing, put, route, settle, workflow};
use super::tests::{click, open_tab};
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    board_edit::tidy,
    board_model::{Flow, Point, trigger_frame},
    model::{NodeKind, RunStatus, WorkflowRun},
    run_graph_model::RunNodeState,
    store::update_runs,
};
use serde_json::json;

fn shown(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    visual.debug_bounds(selector).is_some()
}

fn mode(s: &Setup, visual: &mut VisualTestContext) -> WfMode {
    s.manager.read_with(visual, |m, _| m.wf_mode())
}

fn set_mode(s: &Setup, visual: &mut VisualTestContext, mode: WfMode) {
    do_with(s, visual, |m, cx| m.wf_set_mode(mode, cx));
    settle(s, visual);
}

fn put_run(s: &Setup, run: &WorkflowRun) {
    update_runs(&s.workflows_dir(), &run.workflow_id, |_| {
        Ok((vec![run.clone()], ()))
    })
    .unwrap();
}

fn done_run(w: &Workflow) -> WorkflowRun {
    let mut run = run_of(
        w,
        "done",
        json!({}),
        json!([nr("start", "", "done", json!({}))]),
    );
    run.id = "run-1".into();
    run
}

// ---- the header ---------------------------------------------------------------------------

#[gpui::test]
fn the_header_has_the_breadcrumb_the_toggle_the_modes_and_the_actions(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    let order = [
        "mono-page-back",
        "wf-switcher",
        "wf-enabled-toggle",
        "segmented-wf-mode-edit",
        "segmented-wf-mode-runs",
        "segmented-wf-mode-settings",
        "mono-workflow-run",
        "wf-board-save",
        "wf-tidy",
        "wf-focus",
        "wf-board-more",
    ];
    // Left to right: the breadcrumb, the toggle, the modes, then the actions.
    let xs: Vec<_> = order
        .iter()
        .map(|selector| {
            visual
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector}"))
                .left()
        })
        .collect();
    assert!(xs.windows(2).all(|p| p[0] < p[1]), "{xs:?}");
    assert_eq!(mode(&s, visual), WfMode::Edit);
}

#[gpui::test]
fn the_switcher_lists_the_workflows_and_switches_to_the_one_picked(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut other = workflow(json!({"name": "Second"}));
    other.id = "wf-bbbbbbbb".into();
    put(&s, vec![ticket(json!({})), other]);
    open_tab(&s, cx);
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    let visual = &mut visual;
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(WfRoute::Board(A.into()), cx)
    });
    visual.run_until_parked();
    settle(&s, visual);
    click(visual, "wf-switcher");
    assert!(s.manager.read_with(visual, |m, _| m.picker.is_some()));
    do_with(&s, visual, |m, cx| {
        // As the menu's own answer does: it closes, then the pick is acted on.
        m.picker = None;
        m.board_menu_picked("Switch workflow", "wf-bbbbbbbb".into(), cx)
    });
    settle(&s, visual);
    assert_eq!(
        route(&s, visual),
        Some(WfRoute::Board("wf-bbbbbbbb".into()))
    );
    assert_eq!(on_board(&s, visual, |b| b.base.name.clone()), "Second");
    // Unsaved edits are asked about first.
    rename(&s, visual, "Edited second");
    do_with(&s, visual, |m, cx| {
        m.board_menu_picked("Switch workflow", A.into(), cx)
    });
    settle(&s, visual);
    assert!(shown(visual, "wf-board-leave"));
    assert_eq!(
        route(&s, visual),
        Some(WfRoute::Board("wf-bbbbbbbb".into()))
    );
    click(visual, "wf-board-discard");
    settle(&s, visual);
    assert_eq!(route(&s, visual), Some(WfRoute::Board(A.into())));
    assert_eq!(on_board(&s, visual, |b| b.base.id.clone()), A);
}

#[gpui::test]
fn the_enabled_toggle_flips_the_draft_and_saving_writes_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    assert!(on_board(&s, visual, |b| b.meta.enabled));
    click(visual, "wf-enabled-toggle");
    assert!(!on_board(&s, visual, |b| b.meta.enabled));
    assert!(on_board(&s, visual, |b| b.changed()));
    click(visual, "wf-enabled-toggle");
    assert!(on_board(&s, visual, |b| b.meta.enabled));
    assert!(!on_board(&s, visual, |b| b.changed()));
    click(visual, "wf-enabled-toggle");
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert!(!stored_first(&s).enabled);
}

#[gpui::test]
fn the_enabled_toggle_of_a_flagged_workflow_goes_to_its_review(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(
        &s,
        &ticket(json!({"enabled": false, "needsReview": true})),
        cx,
    );
    let visual = &mut visual;
    click(visual, "wf-enabled-toggle");
    assert!(matches!(
        route(&s, visual),
        Some(WfRoute::Review(_, super::ReviewIntent::Enable))
    ));
}

#[gpui::test]
fn the_mode_control_switches_the_page_and_keeps_the_draft(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = ticket(json!({}));
    put_run(&s, &done_run(&w));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    assert!(shown(visual, "wf-rail") && shown(visual, "wf-graph"));
    rename(&s, visual, "In progress");
    click(visual, "segmented-wf-mode-runs");
    assert_eq!(mode(&s, visual), WfMode::Runs);
    assert_eq!(route(&s, visual), Some(WfRoute::Workflow(A.into())));
    assert!(shown(visual, "wf-runs-list"));
    assert!(!shown(visual, "wf-rail"));
    click(visual, "segmented-wf-mode-settings");
    assert_eq!(mode(&s, visual), WfMode::Settings);
    assert_eq!(route(&s, visual), Some(WfRoute::Board(A.into())));
    assert!(shown(visual, "wf-settings-page"));
    assert!(!shown(visual, "wf-runs-list"));
    click(visual, "segmented-wf-mode-edit");
    assert_eq!(mode(&s, visual), WfMode::Edit);
    assert!(shown(visual, "wf-rail"));
    // The edit made before the first switch is still there, unsaved.
    assert_eq!(on_board(&s, visual, |b| b.meta.name.clone()), "In progress");
    assert!(on_board(&s, visual, |b| b.changed()));
    assert_eq!(stored_first(&s).name, "ClickUp -> Dev -> Review");
}

#[gpui::test]
fn run_opens_a_popover_with_the_inputs_under_its_button(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    click(visual, "mono-workflow-run");
    settle(&s, visual);
    assert!(s.manager.read_with(visual, |m, _| m.wf.gate.is_some()));
    let button = visual.debug_bounds("mono-workflow-run").unwrap();
    let card = visual.debug_bounds("wf-run-gate").expect("the popover");
    // Under the button, not a dialog over everything.
    assert!(card.top() >= button.bottom(), "{card:?} vs {button:?}");
    assert!(
        card.right() <= button.right() + px(2.),
        "{card:?} vs {button:?}"
    );
    assert!(shown(visual, "wf-gate-body"));
    assert!(shown(visual, "wf-gate-submit") && shown(visual, "wf-gate-cancel"));
    // It closes without running.
    click(visual, "wf-gate-cancel");
    assert!(s.manager.read_with(visual, |m, _| m.wf.gate.is_none()));
    assert!(!stored_first(&s).run_requested);
}

#[gpui::test]
fn run_starts_a_workflow_with_nothing_to_ask_at_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = build(
        json!({"inputs": {}, "inputValues": {}}),
        json!([
            {"id": "start", "type": "manual"},
            {"id": "report", "type": "if", "condition": "\"a\" == \"b\""},
        ]),
        json!([{"from": "start", "to": "report"}]),
    );
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    click(visual, "mono-workflow-run");
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.wf.gate.is_none()));
    assert_eq!(super::tests::page_notice(&s, visual), None);
}

#[gpui::test]
fn save_in_the_header_saves_an_edit_made_in_the_settings_mode(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    set_mode(&s, visual, WfMode::Settings);
    type_in(&s, visual, BoardField::Name, "Renamed in settings");
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "Renamed in settings");
    assert!(!on_board(&s, visual, |b| b.changed()));
}

// ---- Focus, Tidy, the rail and the docked inspector ------------------------------------------

#[gpui::test]
fn focus_hides_the_inner_list_and_the_inspector_and_brings_them_back(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = ticket(json!({}));
    put_run(&s, &done_run(&w));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    settle(&s, visual);
    assert!(shown(visual, "wf-rail") && shown(visual, "wf-inspector"));
    let before = visual.debug_bounds("wf-graph").unwrap();
    click(visual, "wf-focus");
    assert!(!shown(visual, "wf-rail") && !shown(visual, "wf-inspector"));
    let after = visual.debug_bounds("wf-graph").unwrap();
    assert!(after.size.width >= before.size.width + px(RAIL_W + INSPECTOR_W - 2.));
    click(visual, "wf-focus");
    assert!(shown(visual, "wf-rail") && shown(visual, "wf-inspector"));

    // The same in the Runs mode, where it hides the runs list and the details.
    set_mode(&s, visual, WfMode::Runs);
    click(visual, "mono-workflow-run-run-1");
    assert!(shown(visual, "wf-runs-list") && shown(visual, "wf-details"));
    click(visual, "wf-focus");
    assert!(!shown(visual, "wf-runs-list") && !shown(visual, "wf-details"));
}

fn at(flow: &Flow, id: &str) -> Point {
    flow.nodes.iter().find(|n| n.id == id).unwrap().position
}

#[gpui::test]
fn tidy_lays_the_board_out_again(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    let (flow, current) = on_board(&s, visual, |b| (b.flow.clone(), b.current()));
    let neat = tidy(&flow, &current);
    do_with(&s, visual, |m, cx| {
        m.board_graph_event(
            &GraphEvent::Drag {
                id: "fetch".into(),
                to: Point { x: 40., y: 900. },
            },
            cx,
        )
    });
    assert_eq!(
        on_board(&s, visual, |b| at(&b.flow, "fetch")),
        Point { x: 40., y: 900. }
    );
    click(visual, "wf-tidy");
    for id in ["start", "fetch", "each", "report"] {
        assert_eq!(
            on_board(&s, visual, |b| at(&b.flow, id)),
            at(&neat, id),
            "{id}"
        );
    }
    // The triggers sit in their one frame.
    assert!(on_board(&s, visual, |b| trigger_frame(&b.flow.nodes).is_some()));
}

#[gpui::test]
fn the_rail_has_a_button_for_each_kind_of_node_and_adds_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    let rail = visual.debug_bounds("wf-rail").unwrap();
    assert_eq!(rail.size.width, px(RAIL_W));
    for id in [
        "wf-add-schedule",
        "wf-add-manual",
        "wf-add-session",
        "wf-add-command",
        "wf-add-if",
        "wf-add-switch",
        "wf-add-forEach",
    ] {
        let b = visual.debug_bounds(id).unwrap_or_else(|| panic!("{id}"));
        assert!(b.left() >= rail.left() && b.right() <= rail.right(), "{id}");
    }
    let before = on_board(&s, visual, |b| b.flow.nodes.len());
    click(visual, "wf-add-if");
    assert_eq!(on_board(&s, visual, |b| b.flow.nodes.len()), before + 1);
    assert!(matches!(
        on_board(&s, visual, |b| b.view.clone()),
        Some(InspectorView::Node(id)) if id.starts_with("check")
    ));
}

#[gpui::test]
fn the_inspector_is_docked_beside_the_canvas_and_shows_the_picked_nodes_form(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    // Nothing picked: the panel is there and says so.
    assert!(shown(visual, "wf-inspector") && shown(visual, "wf-inspector-empty"));
    event(&s, visual, GraphEvent::Click("fetch".into()));
    settle(&s, visual);
    let inspector = visual.debug_bounds("wf-inspector").unwrap();
    let canvas = visual.debug_bounds("wf-graph").unwrap();
    let page = visual.debug_bounds("wf-board").unwrap();
    assert_eq!(inspector.size.width, px(INSPECTOR_W));
    assert_eq!(inspector.right(), page.right());
    // Beside the canvas, not over it.
    assert!(canvas.right() <= inspector.left() + px(1.), "{canvas:?}");
    assert!(shown(visual, "wf-inspector-title"));
    assert!(!shown(visual, "wf-inspector-empty"));
    // It holds the form of the picked node and follows a new pick.
    assert!(s.manager.read_with(visual, |m, _| {
        m.fields
            .contains_key(&BoardField::Prompt("fetch".into()).key())
    }));
    event(&s, visual, GraphEvent::Click("report".into()));
    settle(&s, visual);
    assert!(s.manager.read_with(visual, |m, _| {
        m.fields
            .contains_key(&BoardField::Command("report".into()).key())
    }));
    assert!(shown(visual, "wf-inspector-delete"));
}

// ---- the Problems bar ----------------------------------------------------------------------

#[gpui::test]
fn the_problems_bar_lists_the_issues_and_a_click_selects_the_node(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Session { prompt, .. } = &mut w.nodes[1].kind {
        *prompt = "List {{inputs.nope}}".into();
    }
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    assert!(shown(visual, "wf-board-issues"));
    // The bar sits at the bottom, under the canvas.
    let bar = visual.debug_bounds("wf-board-issues").unwrap();
    let canvas = visual.debug_bounds("wf-graph").unwrap();
    assert!(bar.top() >= canvas.bottom() - px(1.));
    assert!(!shown(visual, "wf-problem-0"));
    click(visual, "wf-board-issues-toggle");
    assert!(shown(visual, "wf-problem-0"));
    assert_eq!(on_board(&s, visual, |b| b.issues.len()), 1);
    // Looking at another node first.
    event(&s, visual, GraphEvent::Click("report".into()));
    click(visual, "wf-problem-0");
    assert_eq!(
        on_board(&s, visual, |b| b.view.clone()),
        Some(InspectorView::Node("fetch".into()))
    );
    assert!(on_board(&s, visual, |b| {
        b.flow.nodes.iter().any(|n| n.id == "fetch" && n.selected)
            && b.flow.nodes.iter().all(|n| n.id == "fetch" || !n.selected)
    }));
    assert_eq!(mode(&s, visual), WfMode::Edit);
}

#[gpui::test]
fn a_problem_of_the_workflow_itself_opens_its_settings(cx: &mut TestAppContext) {
    let s = setup(cx);
    crate::monos::tests::set_probes(&s, cx, |p| {
        *p = crate::workflows::Probes::new().with_probe(listing)
    });
    let w = ticket(json!({
        "decisionModel": {"harness": "claude", "model": "claude:nope"},
    }));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    visual.run_until_parked();
    settle(&s, visual);
    let issues = on_board(&s, visual, |b| b.issues.clone());
    let at = issues
        .iter()
        .position(|i| i.node_id.is_none())
        .unwrap_or_else(|| panic!("{issues:?}"));
    click(visual, "wf-board-issues-toggle");
    let selector: &'static str = Box::leak(format!("wf-problem-{at}").into_boxed_str());
    click(visual, selector);
    assert_eq!(mode(&s, visual), WfMode::Settings);
}

// ---- Settings mode -----------------------------------------------------------------------

#[gpui::test]
fn settings_edits_the_decision_model_the_report_mono_and_the_summary_template_and_saves(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({"enabled": false})), cx);
    let visual = &mut visual;
    set_mode(&s, visual, WfMode::Settings);
    for selector in [
        "wf-board-name",
        "wf-decisions",
        "wf-decision-model",
        "wf-summary",
    ] {
        assert!(shown(visual, selector), "{selector}");
    }
    // The model of the AI decisions, and its effort.
    do_with(&s, visual, |m, cx| {
        m.board_decision_picked(
            monocode_core::provider::Kind::Claude,
            Some("claude-opus-5".into()),
            cx,
        )
    });
    pick(&s, visual, BoardPick::DecisionEffort, "high");
    let model = on_board(&s, visual, |b| b.meta.decision_model.clone()).unwrap();
    assert_eq!(model.harness, "claude");
    assert_eq!(model.model, "claude:opus-5");
    assert_eq!(model.effort.as_deref(), Some("high"));
    settle(&s, visual);
    assert!(shown(visual, "wf-decision-default"));
    // Who gets the summary besides the run page, and its template.
    let mono = s
        .manager
        .read_with(visual, |m, _| m.roster.monos[0].id.clone());
    pick(&s, visual, BoardPick::ReportTo, &mono);
    type_in(
        &s,
        visual,
        BoardField::SummaryTemplate,
        "Done: {{run.id}}\nSecond line",
    );
    assert!(on_board(&s, visual, |b| b.can_save()));
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let stored = stored_first(&s);
    let model = stored.decision_model.clone().unwrap();
    assert_eq!(
        (
            model.harness.as_str(),
            model.model.as_str(),
            model.effort.as_deref()
        ),
        ("claude", "claude:opus-5", Some("high"))
    );
    assert_eq!(
        stored.report_to.as_ref().and_then(|r| r.mono.as_deref()),
        Some(mono.as_str())
    );
    assert_eq!(
        stored.summary_template.as_deref(),
        Some("Done: {{run.id}}\nSecond line")
    );
    // Back to the step's model, to no one and to the usual summary.
    click(visual, "wf-decision-default");
    pick(&s, visual, BoardPick::ReportTo, "");
    type_in(&s, visual, BoardField::SummaryTemplate, "");
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let stored = stored_first(&s);
    assert!(stored.decision_model.is_none());
    assert!(stored.report_to.is_none());
    assert!(stored.summary_template.is_none());
}

#[gpui::test]
fn settings_has_the_name_the_description_the_inputs_and_the_requirements(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    set_mode(&s, visual, WfMode::Settings);
    for selector in ["wf-board-name", "wf-requires", "wf-decisions", "wf-summary"] {
        assert!(shown(visual, selector), "{selector}");
    }
    // Settings is a page of its own: no inspector or rail beside it.
    assert!(!shown(visual, "wf-inspector") && !shown(visual, "wf-rail"));
    assert!(s.manager.read_with(visual, |m, _| {
        m.fields.contains_key(&BoardField::Description.key())
    }));
    // The inputs of the workflow have a row each.
    assert!(s.manager.read_with(visual, |m, _| {
        m.fields
            .contains_key(&BoardField::InputValue("clickupList".into()).key())
    }));
}

// ---- Runs mode -------------------------------------------------------------------------------

fn scene_cards(s: &Setup, visual: &mut VisualTestContext) -> Vec<RunNodeState> {
    let canvas = s
        .manager
        .read_with(visual, |m, _| m.wf.run.canvas.clone().expect("a canvas"));
    canvas.read_with(visual, |c, _| {
        c.scene()
            .nodes
            .iter()
            .filter_map(|n| n.flow.data.run.clone())
            .collect()
    })
}

fn open_runs(s: &Setup, visual: &mut VisualTestContext, route: WfRoute) {
    s.manager
        .update(visual, |m, cx| m.open_workflow_route(route, cx));
    visual.run_until_parked();
    settle(s, visual);
}

#[gpui::test]
fn runs_mode_lists_the_runs_and_draws_the_picked_one_colored_with_round_badges(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let w = linear();
    let mut first = done_run(&w);
    first.started_at = 1_000;
    let mut second = run_of(
        &w,
        "running",
        json!({}),
        json!([
            nr("start", "", "done", json!({})),
            nr(
                "dev",
                "",
                "failed",
                json!({"round": 2, "error": "It broke"})
            ),
            nr("report", "", "pending", json!({})),
        ]),
    );
    second.id = "run-2".into();
    second.started_at = 5_000;
    put(&s, vec![w.clone()]);
    update_runs(&s.workflows_dir(), A, |_| {
        Ok((vec![second.clone(), first.clone()], ()))
    })
    .unwrap();
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_runs(&s, visual, WfRoute::Workflow(A.into()));
    let list = visual.debug_bounds("wf-runs-list").unwrap();
    assert_eq!(list.size.width, px(RUNS_W));
    let (a, b) = (
        visual.debug_bounds("mono-workflow-run-run-2").unwrap(),
        visual.debug_bounds("mono-workflow-run-run-1").unwrap(),
    );
    assert!(a.top() < b.top());
    assert!(a.left() >= list.left() && a.right() <= list.right());
    // Nothing picked yet: the canvas area says so.
    assert!(shown(visual, "wf-run-empty"));
    click(visual, "mono-workflow-run-run-2");
    assert_eq!(
        route(&s, visual),
        Some(WfRoute::Run(A.into(), "run-2".into()))
    );
    // The canvas is colored by what the run did with each node.
    let mut statuses: Vec<_> = scene_cards(&s, visual)
        .iter()
        .map(|c| (c.node_run_key.clone(), c.status.name(), c.round))
        .collect();
    statuses.sort();
    assert_eq!(
        statuses,
        vec![
            ("dev|".to_string(), "failed", 2),
            ("report|".to_string(), "pending", 1),
            ("start|".to_string(), "done", 1),
        ]
    );
    // A node that ran again carries a round badge, one that ran once does not.
    assert!(shown(visual, "wf-round-badge-dev"));
    assert!(!shown(visual, "wf-round-badge-start"));
    // The run's facts are over the canvas, and the details beside it.
    for selector in ["wf-run-meta", "wf-run-caller", "wf-run-top", "wf-details"] {
        assert!(shown(visual, selector), "{selector}");
    }
    // Picking a node shows what it did in the docked inspector.
    let canvas = s
        .manager
        .read_with(visual, |m, _| m.wf.run.canvas.clone().unwrap());
    canvas.update(visual, |_, cx| cx.emit(GraphEvent::Select("dev|".into())));
    visual.run_until_parked();
    settle(&s, visual);
    let heading = s.manager.read_with(visual, |m, _| {
        m.wf.run.details.as_ref().map(|d| d.heading.clone())
    });
    assert!(heading.is_some());
    let details = visual.debug_bounds("wf-details").unwrap();
    let page = visual.debug_bounds("wf-board").unwrap();
    assert_eq!(details.size.width, px(INSPECTOR_W));
    assert_eq!(details.right(), page.right());
    assert!(shown(visual, "wf-details-body"));
}

#[gpui::test]
fn a_long_summary_scrolls_in_its_own_box_and_leaves_the_canvas_its_room(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = linear();
    let mut run = done_run(&w);
    run.summary = Some(monocode_core::mono::workflows::model::RunSummary {
        title: "Done".into(),
        text: (0..60)
            .map(|i| format!("Line {i} of a very long summary"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    });
    put(&s, vec![w.clone()]);
    put_run(&s, &run);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_runs(&s, visual, WfRoute::Run(A.into(), "run-1".into()));
    let summary = visual.debug_bounds("wf-run-summary").unwrap();
    assert!(summary.size.height <= px(140.), "{summary:?}");
    let canvas = visual.debug_bounds("wf-graph").unwrap();
    assert!(canvas.size.height >= px(200.), "{canvas:?}");
    // The run's other facts are still there.
    assert!(shown(visual, "wf-run-caller") && shown(visual, "wf-run-started"));
}

#[gpui::test]
fn runs_mode_keeps_resume_and_the_changed_notice_of_the_run_page(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = linear();
    let mut run = done_run(&w);
    run.status = RunStatus::Interrupted;
    // The workflow is edited after the run took its copy.
    let mut edited = w.clone();
    edited.edges.clear();
    put(&s, vec![edited]);
    put_run(&s, &run);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_runs(&s, visual, WfRoute::Run(A.into(), "run-1".into()));
    assert!(shown(visual, "wf-run-resume"));
    assert!(shown(visual, "wf-run-changed"));
}

#[gpui::test]
fn for_each_lanes_are_titled_rows_stacked_top_to_bottom(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![each_workflow()]);
    let mut run = each_run(4);
    run.id = "run-1".into();
    put_run(&s, &run);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_runs(&s, visual, WfRoute::Run(A.into(), "run-1".into()));
    let canvas = s
        .manager
        .read_with(visual, |m, _| m.wf.run.canvas.clone().unwrap());
    let (mut cards, mut bars) = canvas.read_with(visual, |c, _| {
        let scene = c.scene();
        let cards: Vec<(f32, f32)> = scene
            .nodes
            .iter()
            .filter(|n| n.flow.id.starts_with("dev@"))
            .map(|n| (n.rect.x, n.rect.y))
            .collect();
        let bars: Vec<(f32, f32)> = scene
            .bars
            .iter()
            .filter(|b| matches!(b.kind, BarKind::Lane { .. }))
            .map(|b| (b.rect.x, b.rect.y))
            .collect();
        (cards, bars)
    });
    assert_eq!(cards.len(), 4);
    cards.sort_by(|a, b| a.1.total_cmp(&b.1));
    // One column of cards, going down.
    assert!(
        cards
            .windows(2)
            .all(|p| p[0].0 == p[1].0 && p[0].1 < p[1].1)
    );
    // And a title above each row, in the same order.
    assert_eq!(bars.len(), 4);
    bars.sort_by(|a, b| a.1.total_cmp(&b.1));
    assert!(bars.windows(2).all(|p| p[0].0 == p[1].0 && p[0].1 < p[1].1));
    for (bar, card) in bars.iter().zip(&cards) {
        assert!(bar.1 < card.1, "a title is above its row");
        assert!(card.1 - bar.1 <= 40.);
    }
}

// ---- the edge of the trigger frame -------------------------------------------------------------

#[gpui::test]
fn the_trigger_frame_edge_selects_and_deletes_every_trigger_edge_to_its_target(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let w = build(
        json!({}),
        json!([
            {"id": "start", "type": "manual"},
            {"id": "second", "type": "manual"},
            {"id": "report", "type": "command", "project": "{{inputs.project}}", "command": "true"},
        ]),
        json!([{"from": "start", "to": "report"}, {"from": "second", "to": "report"}]),
    );
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    let canvas = on_board(&s, visual, |b| b.canvas.clone());
    let drawn: Vec<String> = canvas.read_with(visual, |c, _| {
        c.scene().edges.iter().map(|e| e.id.clone()).collect()
    });
    // Two triggers lead to the step; one edge is drawn, from the frame.
    assert_eq!(drawn.len(), 1, "{drawn:?}");
    event(&s, visual, GraphEvent::EdgeClick(drawn[0].clone()));
    assert_eq!(
        on_board(&s, visual, |b| b
            .flow
            .edges
            .iter()
            .filter(|e| e.selected)
            .count()),
        2
    );
    event(&s, visual, GraphEvent::Delete);
    assert!(on_board(&s, visual, |b| b.flow.edges.is_empty()));
    assert_eq!(on_board(&s, visual, |b| b.flow.nodes.len()), 3);
}
