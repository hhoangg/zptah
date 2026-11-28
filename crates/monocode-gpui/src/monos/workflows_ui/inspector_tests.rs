//! `NodeInspector.test.ts` and the fields of the inspector.
use super::board::{BoardField, BoardPick, BoardState};
use super::board_tests::*;
use super::graph::GraphEvent;
use super::tests::{click, listing, settle};
use crate::monos::tests::{Setup, setup};
use gpui::{Focusable, TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    board_model::NodeType,
    model::{Checkout, Decide, InputValue, NodeKind},
};
use serde_json::json;

// ---- NodeInspector.test.ts -------------------------------------------------------------------

#[gpui::test]
fn the_in_report_button_stores_the_opposite_of_the_effective_value(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    // A string field is in the report, a number field is not.
    click_in_inspector(visual, "wf-output-report-pr_url");
    let node = node_of(&s, visual, "dev");
    let NodeKind::Session { output, .. } = &node.kind else {
        panic!()
    };
    assert_eq!(output.get("pr_url").unwrap().in_report, Some(false));
    click_in_inspector(visual, "wf-output-report-count");
    let node = node_of(&s, visual, "dev");
    let NodeKind::Session { output, .. } = &node.kind else {
        panic!()
    };
    assert_eq!(output.get("count").unwrap().in_report, Some(true));
}

#[gpui::test]
fn the_reuse_switch_shows_on_sessions_in_a_loop_body_only(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    for id in ["dev", "review"] {
        event(&s, visual, GraphEvent::Click(id.into()));
        assert!(visual.debug_bounds("wf-reuse-session").is_some(), "{id}");
    }
    event(&s, visual, GraphEvent::Click("fetch".into()));
    assert!(visual.debug_bounds("wf-reuse-session").is_none());
    event(&s, visual, GraphEvent::Click("dev".into()));
    click(visual, "wf-reuse-session");
    let NodeKind::Session { reuse_session, .. } = node_of(&s, visual, "dev").kind else {
        panic!()
    };
    assert_eq!(reuse_session, Some(false));
}

#[gpui::test]
fn a_cloud_step_shows_where_it_runs_and_an_edit_keeps_its_target(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::{
        model::{SessionTarget, TargetInstance},
        ordered::{Json, OrderedMap},
    };
    let s = setup(cx);
    let mut w = ticket(json!({}));
    let mut options = OrderedMap::new();
    options.insert("branch", Json::String("trunk".into()));
    let cloud = SessionTarget {
        kind: "codespace".into(),
        instance: TargetInstance::New,
        options,
    };
    if let NodeKind::Session { target, .. } = &mut w.nodes[1].kind {
        *target = Some(cloud.clone());
    }
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    assert!(visual.debug_bounds("wf-target").is_some());
    // Edits in place (the prompt, the limit, the model) leave the target as it is.
    type_in(
        &s,
        visual,
        BoardField::Prompt("fetch".into()),
        "List them all.",
    );
    type_in(&s, visual, BoardField::Limit("fetch".into()), "30");
    let claude = monocode_core::provider::Kind::Claude;
    do_with(&s, visual, |m, cx| {
        m.board_model_picked("fetch", claude, Some("claude-opus-4-6".into()), cx)
    });
    let NodeKind::Session {
        target,
        prompt,
        timeout_minutes,
        ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(prompt, "List them all.");
    assert_eq!(timeout_minutes, Some(30.));
    assert_eq!(target, Some(cloud));
    // A step on this computer has no such row.
    event(&s, visual, GraphEvent::Click("dev".into()));
    assert!(visual.debug_bounds("wf-target").is_none());
}

#[gpui::test]
fn a_cloud_step_shows_its_account_next_to_where_it_runs_and_an_edit_keeps_it(
    cx: &mut TestAppContext,
) {
    use monocode_core::mono::workflows::{
        model::{SessionTarget, TargetInstance},
        ordered::OrderedMap,
    };
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Session {
        target, account, ..
    } = &mut w.nodes[1].kind
    {
        *target = Some(SessionTarget {
            kind: "sandbox".into(),
            instance: TargetInstance::New,
            options: OrderedMap::new(),
        });
        *account = Some("Work".into());
    }
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    assert!(visual.debug_bounds("wf-target").is_some());
    assert!(visual.debug_bounds("wf-account").is_some());
    // Edits in place leave the account as it is.
    type_in(
        &s,
        visual,
        BoardField::Prompt("fetch".into()),
        "List them all.",
    );
    let NodeKind::Session {
        account, prompt, ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(prompt, "List them all.");
    assert_eq!(account.as_deref(), Some("Work"));
    // A step without one has no such row, and neither has a step on this computer.
    event(&s, visual, GraphEvent::Click("dev".into()));
    assert!(visual.debug_bounds("wf-account").is_none());
}

#[gpui::test]
fn picking_a_model_writes_its_harness_and_model_and_drops_the_effort(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Session { effort, .. } = &mut w.nodes[1].kind {
        *effort = Some("high".into());
    }
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    // The picker's answer, as the list sends it.
    let claude = monocode_core::provider::Kind::Claude;
    do_with(&s, visual, |m, cx| {
        m.board_model_picked("fetch", claude, Some("claude-opus-4-6".into()), cx)
    });
    let NodeKind::Session {
        harness,
        model,
        effort,
        ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(harness.as_deref(), Some("claude"));
    // React's spelling: only the `claude-` prefix comes off.
    assert_eq!(model.as_deref(), Some("claude:opus-4-6"));
    assert_eq!(effort, None);
    do_with(&s, visual, |m, cx| {
        m.board_model_picked("fetch", claude, Some("claude-opus-5-5".into()), cx)
    });
    let NodeKind::Session { model, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(model.as_deref(), Some("claude:opus-5-5"));
}

#[gpui::test]
fn picking_an_effort_fixes_the_model_it_was_chosen_for_and_use_mono_default_clears_them(
    cx: &mut TestAppContext,
) {
    use monocode_core::{catalog, mono::workflows::models::Probe, provider::Kind};
    fn efforts(_: Kind) -> Probe {
        Probe {
            installed: true,
            models: Some(vec![catalog::Model {
                id: "claude-sonnet-5".into(),
                name: "Sonnet 5".into(),
                alias_target: None,
                efforts: vec!["low".into(), "high".into(), "xhigh".into()],
                default_effort: None,
                fast: false,
                thinking: false,
                default: true,
                tiers: Vec::new(),
                controls: Vec::new(),
            }]),
        }
    }
    let s = setup(cx);
    crate::monos::tests::set_probes(&s, cx, |p| {
        *p = crate::workflows::Probes::new().with_probe(efforts)
    });
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    visual.run_until_parked();
    pick(&s, visual, BoardPick::Effort("fetch".into()), "xhigh");
    let NodeKind::Session {
        harness,
        model,
        effort,
        ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(harness.as_deref(), Some("claude"));
    assert_eq!(model.as_deref(), Some("claude:sonnet-5"));
    assert_eq!(effort.as_deref(), Some("xhigh"));
    click(visual, "wf-model-default");
    let NodeKind::Session {
        harness,
        model,
        effort,
        ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!((harness, model, effort), (None, None, None));
    assert!(visual.debug_bounds("wf-model-default").is_none());
}

#[gpui::test]
fn the_inspector_says_why_a_stored_model_is_not_available(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    if let NodeKind::Session { model, .. } = &mut w.nodes[1].kind {
        *model = Some("claude-sonnet-5-5".into());
    }
    crate::monos::tests::set_probes(&s, cx, |p| {
        *p = crate::workflows::Probes::new().with_probe(listing)
    });
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    visual.run_until_parked();
    assert!(visual.debug_bounds("wf-model-issue").is_some());
    assert!(visual.debug_bounds("wf-inspector-issues").is_some());
    // A node that sets nothing has no hint.
    event(&s, visual, GraphEvent::Click("dev".into()));
    assert!(visual.debug_bounds("wf-model-issue").is_none());
}

#[gpui::test]
fn the_checkout_nest_and_if_fields_write_their_values(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    pick(
        &s,
        visual,
        BoardPick::Checkout("dev".into()),
        "worktree:fetch",
    );
    let NodeKind::Session { checkout, .. } = node_of(&s, visual, "dev").kind else {
        panic!()
    };
    assert_eq!(checkout, Checkout::WorktreeOf("fetch".into()));
    pick(&s, visual, BoardPick::NestUnder("review".into()), "");
    let NodeKind::Session { nest_under, .. } = node_of(&s, visual, "review").kind else {
        panic!()
    };
    assert_eq!(nest_under, None);
    do_with(&s, visual, |m, cx| m.board_add(NodeType::If, cx));
    let check = node_ids(&s, visual)
        .into_iter()
        .find(|id| id.starts_with("check"))
        .unwrap();
    pick(&s, visual, BoardPick::IfDecider(check.clone()), "condition");
    let NodeKind::If {
        condition, decide, ..
    } = node_of(&s, visual, &check).kind
    else {
        panic!()
    };
    assert_eq!((condition.as_deref(), decide), (Some(""), None));
    type_in(
        &s,
        visual,
        BoardField::Condition(check.clone()),
        "{{a.b}} == 1",
    );
    let NodeKind::If { condition, .. } = node_of(&s, visual, &check).kind else {
        panic!()
    };
    assert_eq!(condition.as_deref(), Some("{{a.b}} == 1"));
}

fn commit(s: &Setup, visual: &mut VisualTestContext, spec: BoardField) {
    do_with(s, visual, |m, cx| m.board_commit(&spec, cx));
}

fn add_switch(s: &Setup, visual: &mut VisualTestContext) -> String {
    do_with(s, visual, |m, cx| m.board_add(NodeType::Switch, cx));
    on_board(s, visual, |b| {
        b.flow
            .nodes
            .iter()
            .find(|n| matches!(n.data.node.kind, NodeKind::Switch { .. }))
            .map(|n| n.id.clone())
            .expect("a switch")
    })
}

fn cases_of(s: &Setup, visual: &mut VisualTestContext, id: &str) -> Vec<String> {
    let NodeKind::Switch { cases, .. } = node_of(s, visual, id).kind else {
        panic!()
    };
    cases
}

#[gpui::test]
fn the_switch_decider_value_and_question_write_their_values(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    let route = add_switch(&s, visual);
    pick(&s, visual, BoardPick::SwitchDecider(route.clone()), "value");
    let NodeKind::Switch { value, decide, .. } = node_of(&s, visual, &route).kind else {
        panic!()
    };
    assert_eq!((value.as_deref(), decide), (Some(""), None));
    type_in(
        &s,
        visual,
        BoardField::SwitchValue(route.clone()),
        "{{review.verdict}}",
    );
    let NodeKind::Switch { value, .. } = node_of(&s, visual, &route).kind else {
        panic!()
    };
    assert_eq!(value.as_deref(), Some("{{review.verdict}}"));
    // AI decides drops the value and shows the question.
    pick(&s, visual, BoardPick::SwitchDecider(route.clone()), "mono");
    let NodeKind::Switch { value, decide, .. } = node_of(&s, visual, &route).kind else {
        panic!()
    };
    assert_eq!((value, decide), (None, Some(Decide::Ai)));
    type_in(
        &s,
        visual,
        BoardField::SwitchQuestion(route.clone()),
        "Which kind is it?",
    );
    let NodeKind::Switch { question, .. } = node_of(&s, visual, &route).kind else {
        panic!()
    };
    assert_eq!(question.as_deref(), Some("Which kind is it?"));
}

#[gpui::test]
fn switch_cases_can_be_added_renamed_and_removed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    let route = add_switch(&s, visual);
    let start = cases_of(&s, visual, &route);
    click(visual, "wf-case-add");
    click(visual, "wf-case-add");
    let cases = cases_of(&s, visual, &route);
    assert_eq!(cases.len(), start.len() + 2);
    let (first, second) = (
        cases[cases.len() - 2].clone(),
        cases[cases.len() - 1].clone(),
    );
    assert_ne!(first, second);
    // A line leaving by the first new case follows it when it is renamed. The step it goes to is
    // a new one: the existing ones lead into the switch, and a line back would be a loop.
    do_with(&s, visual, |m, cx| m.board_add(NodeType::Command, cx));
    let target = on_board(&s, visual, |b| {
        b.flow
            .nodes
            .iter()
            .find(|n| matches!(n.data.node.kind, NodeKind::Command { .. }) && n.id != "report")
            .map(|n| n.id.clone())
            .expect("a command")
    });
    event(
        &s,
        visual,
        GraphEvent::Connect {
            source: route.clone(),
            handle: Some(first.clone()),
            target,
            at: at(),
        },
    );
    let leaves_by = |s: &Setup, visual: &mut VisualTestContext, case: &str| {
        let (route, case) = (route.clone(), case.to_string());
        on_board(s, visual, move |b| {
            b.flow
                .edges
                .iter()
                .any(|e| e.source == route && e.source_handle.as_deref() == Some(case.as_str()))
        })
    };
    assert!(leaves_by(&s, visual, &first));
    // The new step took the inspector; back to the switch.
    let again = route.clone();
    do_with(&s, visual, move |m, cx| {
        m.board_view(Some(super::board::InspectorView::Node(again)), cx)
    });
    type_in(
        &s,
        visual,
        BoardField::CaseName(route.clone(), first.clone()),
        "bug",
    );
    commit(
        &s,
        visual,
        BoardField::CaseName(route.clone(), first.clone()),
    );
    let cases = cases_of(&s, visual, &route);
    assert_eq!(cases[cases.len() - 2], "bug");
    assert!(leaves_by(&s, visual, "bug") && !leaves_by(&s, visual, &first));
    // Blank, reserved and taken names go back to what the case was.
    for bad in ["", "  ", "default", second.as_str()] {
        type_in(
            &s,
            visual,
            BoardField::CaseName(route.clone(), "bug".into()),
            bad,
        );
        commit(
            &s,
            visual,
            BoardField::CaseName(route.clone(), "bug".into()),
        );
        assert_eq!(cases_of(&s, visual, &route), cases, "{bad:?}");
    }
    // Removing a case takes the lines that left by it.
    click(visual, "wf-case-remove-bug");
    assert!(!cases_of(&s, visual, &route).contains(&"bug".to_string()));
    assert!(!leaves_by(&s, visual, "bug"));
    assert_eq!(cases_of(&s, visual, &route).len(), start.len() + 1);
    // The last case cannot be removed.
    while cases_of(&s, visual, &route).len() > 1 {
        let case = cases_of(&s, visual, &route)[0].clone();
        click(
            visual,
            Box::leak(format!("wf-case-remove-{case}").into_boxed_str()),
        );
    }
    let last = cases_of(&s, visual, &route)[0].clone();
    assert!(
        visual
            .debug_bounds(Box::leak(format!("wf-case-remove-{last}").into_boxed_str()))
            .is_none()
    );
}

#[gpui::test]
fn a_time_limit_and_the_for_each_numbers_follow_what_is_typed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    type_in(&s, visual, BoardField::Limit("fetch".into()), "45");
    let NodeKind::Session {
        timeout_minutes, ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(timeout_minutes, Some(45.0));
    type_in(&s, visual, BoardField::Limit("fetch".into()), "");
    let NodeKind::Session {
        timeout_minutes, ..
    } = node_of(&s, visual, "fetch").kind
    else {
        panic!()
    };
    assert_eq!(timeout_minutes, None);
    event(&s, visual, GraphEvent::Click("each".into()));
    type_in(&s, visual, BoardField::AtOnce("each".into()), "5");
    let NodeKind::ForEach { max_parallel, .. } = node_of(&s, visual, "each").kind else {
        panic!()
    };
    assert_eq!(max_parallel, Some(5.0));
}

#[gpui::test]
fn a_new_output_field_can_be_renamed_and_removed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    click_in_inspector(visual, "wf-output-add");
    let NodeKind::Session { output, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(
        output.keys().cloned().collect::<Vec<_>>(),
        vec!["tickets", "field"]
    );
    // A name that is taken or unusable goes back; a usable one renames in place.
    type_in(
        &s,
        visual,
        BoardField::OutputName("fetch".into(), "field".into()),
        "tickets",
    );
    do_with(&s, visual, |m, cx| {
        m.board_graph_event(&GraphEvent::PaneClick, cx);
    });
    event(&s, visual, GraphEvent::Click("fetch".into()));
    click_in_inspector(visual, "wf-output-remove-field");
    let NodeKind::Session { output, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(output.keys().cloned().collect::<Vec<_>>(), vec!["tickets"]);
}

#[gpui::test]
fn a_session_step_edits_its_surface_and_mode_and_lists_its_built_in_outputs(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    let shown =
        |visual: &mut VisualTestContext, id: &'static str| visual.debug_bounds(id).is_some();
    assert!(shown(visual, "segmented-wf-surface-chat"));
    assert!(shown(visual, "segmented-wf-mode-of-step-waitForMe"));
    assert!(!shown(visual, "wf-surface-reason"));
    let NodeKind::Session { surface, mode, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!((surface, mode), (None, None));
    // Chat steps list the reply and the git facts as built-ins.
    assert!(shown(visual, "wf-builtin-outputs"));
    click(visual, "segmented-wf-mode-of-step-waitForMe");
    click(visual, "segmented-wf-surface-terminal");
    let NodeKind::Session { surface, mode, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(surface.as_deref(), Some("terminal"));
    assert_eq!(mode.as_deref(), Some("waitForMe"));
    // The built-ins of a terminal step have no reply.
    use monocode_core::mono::workflows::{model::SessionSurface, output::session_builtins};
    assert!(!session_builtins(SessionSurface::Terminal, false).contains(&"reply"));
    assert_eq!(
        session_builtins(SessionSurface::Chat, true),
        ["reply", "sessionId"]
    );
    // Back to the defaults: nothing is written for them.
    click(visual, "segmented-wf-mode-of-step-auto");
    click(visual, "segmented-wf-surface-chat");
    let NodeKind::Session { surface, mode, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!((surface, mode), (None, None));
}

#[gpui::test]
fn terminal_is_disabled_with_the_reason_when_the_step_cannot_run_in_a_terminal(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| {
        m.board_node_edit(
            "fetch",
            |n| {
                if let NodeKind::Session { harness, model, .. } = &mut n.kind {
                    *harness = Some("codex".into());
                    *model = None;
                }
            },
            cx,
        )
    });
    event(&s, visual, GraphEvent::Click("fetch".into()));
    assert!(visual.debug_bounds("wf-surface-reason").is_some());
    // Pressing it changes nothing.
    click(visual, "segmented-wf-surface-terminal");
    let NodeKind::Session { surface, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(surface, None);
    // A step that already is a terminal one can still be put back to chat.
    do_with(&s, visual, |m, cx| {
        m.board_node_edit(
            "fetch",
            |n| {
                if let NodeKind::Session { surface, .. } = &mut n.kind {
                    *surface = Some("terminal".into());
                }
            },
            cx,
        )
    });
    click(visual, "segmented-wf-surface-chat");
    let NodeKind::Session { surface, .. } = node_of(&s, visual, "fetch").kind else {
        panic!()
    };
    assert_eq!(surface, None);
}

#[gpui::test]
fn an_output_field_named_like_a_built_in_is_flagged_on_the_step(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("fetch".into()));
    click_in_inspector(visual, "wf-output-add");
    type_in(
        &s,
        visual,
        BoardField::OutputName("fetch".into(), "field".into()),
        "reply",
    );
    commit(
        &s,
        visual,
        BoardField::OutputName("fetch".into(), "field".into()),
    );
    let issues = on_board(&s, visual, |b| b.issues.clone());
    assert!(
        issues.iter().any(|i| i.node_id.as_deref() == Some("fetch")
            && i.message.contains("\"reply\" is a built-in output")),
        "{issues:?}"
    );
    assert!(visual.debug_bounds("wf-inspector-issues").is_some());
}

#[gpui::test]
fn a_project_chosen_for_a_step_is_held_by_an_input(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    pick(&s, visual, BoardPick::Project("review".into()), "/work/web");
    let NodeKind::Session { project, .. } = node_of(&s, visual, "review").kind else {
        panic!()
    };
    assert_eq!(project, "{{inputs.web}}");
    let (inputs, values) = on_board(&s, visual, |b| {
        (b.meta.inputs.clone(), b.meta.input_values.clone())
    });
    assert!(inputs.contains_key("web") && inputs.contains_key("project"));
    assert_eq!(
        values.get("web"),
        Some(&InputValue::String("/work/web".into()))
    );
    assert!(on_board(&s, visual, |b| b.changed()));
}

#[gpui::test]
fn the_settings_edit_the_description_and_the_inputs(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    open_settings(&s, visual);
    assert!(visual.debug_bounds("wf-settings-page").is_some());
    type_in(&s, visual, BoardField::Description, "What it is for.");
    assert_eq!(
        on_board(&s, visual, |b| b.meta.description.clone()).as_deref(),
        Some("What it is for.")
    );
    // Declare a number input, give it a value that does not fit, then one that does.
    pick(&s, visual, BoardPick::NewInputType, "number");
    type_in(&s, visual, BoardField::NewInput, "retries");
    do_with(&s, visual, |m, cx| {
        m.board_add_input("retries", "number", cx)
    });
    assert!(on_board(&s, visual, |b| b
        .meta
        .inputs
        .contains_key("retries")));
    // Missing value: a problem the Save gate shows.
    assert!(on_board(&s, visual, |b| b
        .issues
        .iter()
        .any(|i| i.message.contains("retries"))));
    type_in(&s, visual, BoardField::InputValue("retries".into()), "3");
    do_with(&s, visual, |m, cx| {
        m.board_graph_event(&GraphEvent::PaneClick, cx);
    });
    s.manager.update(visual, |m, cx| {
        let input =
            m.wf.board
                .as_ref()
                .unwrap()
                .meta
                .inputs
                .get("retries")
                .cloned()
                .unwrap();
        m.board_set_input("retries", &input, Some(InputValue::Number(3.0)), cx);
    });
    visual.run_until_parked();
    assert!(!on_board(&s, visual, |b| b
        .issues
        .iter()
        .any(|i| i.message.contains("retries"))));
    // Removing it takes the value too.
    do_with(&s, visual, |m, cx| m.board_remove_input("retries", cx));
    assert!(!on_board(&s, visual, |b| b
        .meta
        .input_values
        .contains_key("retries")));
}

#[gpui::test]
fn the_enabled_switch_saves_at_once_when_nothing_else_is_unsaved(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    assert!(stored_first(&s).enabled);
    do_with(&s, visual, |m, cx| m.board_toggle_enabled(cx));
    assert!(!on_board(&s, visual, |b| b.meta.enabled));
    assert!(!stored_first(&s).enabled);
    assert!(!on_board(&s, visual, BoardState::changed));
}

#[gpui::test]
fn the_enabled_switch_stays_a_draft_edit_beside_other_unsaved_edits(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Renamed");
    do_with(&s, visual, |m, cx| m.board_toggle_enabled(cx));
    assert!(!on_board(&s, visual, |b| b.meta.enabled));
    assert!(stored_first(&s).enabled);
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert!(!stored_first(&s).enabled);
}

#[gpui::test]
fn the_header_shows_save_in_runs_mode_once_the_draft_changed(cx: &mut TestAppContext) {
    use super::layout::WfMode;
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    rename(&s, visual, "Renamed");
    do_with(&s, visual, |m, cx| m.wf_set_mode(WfMode::Runs, cx));
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-board-save").is_some());
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "Renamed");
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-board-save").is_none());
}

#[gpui::test]
fn a_default_decision_model_pick_never_stores_an_invalid_model(cx: &mut TestAppContext) {
    use monocode_core::provider::Kind;
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| {
        m.board_decision_picked(Kind::Claude, Some("claude-sonnet-5".into()), cx)
    });
    let model = on_board(&s, visual, |b| b.meta.decision_model.clone()).unwrap();
    assert_eq!(
        (model.harness.as_str(), model.model.as_deref()),
        ("claude", Some("claude:sonnet-5"))
    );
    // A harness with no bundled default has no id to store: only the harness is kept, and the
    // run asks that harness's default model (D44).
    do_with(&s, visual, |m, cx| {
        m.board_decision_picked(Kind::Claude, None, cx)
    });
    let model = on_board(&s, visual, |b| b.meta.decision_model.clone()).unwrap();
    assert_eq!(
        (
            model.harness.as_str(),
            model.model.clone(),
            model.effort.clone()
        ),
        ("claude", None, None)
    );
    // And the saved file keeps it as `{harness}` with no model key.
    let saved = serde_json::to_value(&model).unwrap();
    assert_eq!(saved, json!({"harness": "claude"}));
    // Pi has one, and the file stores it.
    do_with(&s, visual, |m, cx| {
        m.board_decision_picked(Kind::Pi, None, cx)
    });
    let model = on_board(&s, visual, |b| b.meta.decision_model.clone()).unwrap();
    assert_eq!(model.model.as_deref(), Some("pi:default"));
}

#[gpui::test]
fn a_schedule_trigger_edits_its_kind_hours_and_time(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("start".into()));
    pick(
        &s,
        visual,
        BoardPick::ScheduleKind("start".into()),
        "interval",
    );
    let NodeKind::Schedule { schedule } = node_of(&s, visual, "start").kind else {
        panic!()
    };
    assert_eq!(
        (schedule.schedule_kind.as_str(), schedule.interval_hours),
        ("interval", Some(4))
    );
    pick(&s, visual, BoardPick::ScheduleHours("start".into()), "6");
    type_in(&s, visual, BoardField::Time("start".into()), "08:30");
    let NodeKind::Schedule { schedule } = node_of(&s, visual, "start").kind else {
        panic!()
    };
    assert_eq!(
        (schedule.interval_hours, schedule.time.as_str()),
        (Some(6), "08:30")
    );
    pick(&s, visual, BoardPick::ScheduleKind("start".into()), "daily");
    let NodeKind::Schedule { schedule } = node_of(&s, visual, "start").kind else {
        panic!()
    };
    assert_eq!(schedule.interval_hours, None);
}

fn focus_field(s: &Setup, visual: &mut VisualTestContext, spec: BoardField) {
    let handle = s
        .manager
        .read_with(visual, |m, cx| m.fields[&spec.key()].focus_handle(cx));
    visual.refresh().unwrap();
    // Focus listeners (blur) only hear a window that is the active one.
    visual.update(|window, _| window.activate_window());
    visual.run_until_parked();
    visual.update(|window, cx| window.focus(&handle, cx));
    visual.run_until_parked();
    // Focus events are delivered with the next frame.
    visual.refresh().unwrap();
    assert!(
        visual.update(|window, _| handle.is_focused(window)),
        "{spec:?}"
    );
}

fn open_settings(s: &Setup, visual: &mut VisualTestContext) {
    do_with(s, visual, |m, cx| {
        m.wf_set_mode(super::layout::WfMode::Settings, cx)
    });
    visual.refresh().unwrap();
}

#[gpui::test]
fn a_new_input_is_declared_by_enter_or_plus_and_never_by_leaving_the_field(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    open_settings(&s, visual);
    let declared = |s: &Setup, visual: &mut VisualTestContext, name: &str| {
        on_board(s, visual, |b| b.meta.inputs.get(name).map(|i| i.ty))
    };
    type_in(&s, visual, BoardField::NewInput, "retries");
    focus_field(&s, visual, BoardField::NewInput);
    // Opening the type picker takes the focus off the field: that declares nothing.
    click(visual, "wf-pick:NewInputType");
    assert_eq!(declared(&s, visual, "retries"), None);
    pick(&s, visual, BoardPick::NewInputType, "number");
    s.manager.update(visual, |m, cx| {
        m.picker = None;
        cx.notify();
    });
    visual.refresh().unwrap();
    assert_eq!(declared(&s, visual, "retries"), None);
    assert!(!on_board(&s, visual, |b| b.changed()));
    // Leaving the page does not declare it either.
    do_with(&s, visual, |m, cx| {
        m.wf_set_mode(super::layout::WfMode::Edit, cx)
    });
    visual.refresh().unwrap();
    assert_eq!(declared(&s, visual, "retries"), None);
    open_settings(&s, visual);
    // Enter does, with the type chosen.
    focus_field(&s, visual, BoardField::NewInput);
    visual.simulate_keystrokes("enter");
    assert_eq!(
        declared(&s, visual, "retries"),
        Some(monocode_core::mono::workflows::model::InputType::Number)
    );
    // And so does +.
    visual.refresh().unwrap();
    type_in(&s, visual, BoardField::NewInput, "other");
    click(visual, "wf-input-add");
    assert!(declared(&s, visual, "other").is_some());
}

#[gpui::test]
fn leaving_an_input_value_as_it_was_shown_changes_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = ticket(json!({
        "inputs": {"project": {"type": "project"}, "clickupList": {"type": "string"}, "note": {"type": "string"}},
    }));
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    open_settings(&s, visual);
    assert!(!on_board(&s, visual, |b| b.changed()));
    // Focus the empty field and leave it: still clean, still no value.
    focus_field(&s, visual, BoardField::InputValue("note".into()));
    click(visual, "wf-board-name");
    assert!(!on_board(&s, visual, |b| b.changed()));
    assert!(!on_board(&s, visual, |b| b
        .meta
        .input_values
        .contains_key("note")));
    // A value that was typed is kept when the field is left.
    type_in(&s, visual, BoardField::InputValue("note".into()), "hello");
    focus_field(&s, visual, BoardField::InputValue("note".into()));
    click(visual, "wf-board-name");
    assert_eq!(
        on_board(&s, visual, |b| b.meta.input_values.get("note").cloned()),
        Some(InputValue::String("hello".into()))
    );
    assert!(on_board(&s, visual, |b| b.changed()));
}

#[gpui::test]
fn text_that_is_not_a_number_shows_the_problem_and_leaves_the_draft(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| {
        m.board_add_input("retries", "number", cx)
    });
    open_settings(&s, visual);
    let before = on_board(&s, visual, |b| (b.meta.input_values.clone(), b.current()));
    type_in(
        &s,
        visual,
        BoardField::InputValue("retries".into()),
        "not a number",
    );
    focus_field(&s, visual, BoardField::InputValue("retries".into()));
    click(visual, "wf-board-name");
    assert!(on_board(&s, visual, |b| b.input_problem.is_some()));
    let after = on_board(&s, visual, |b| (b.meta.input_values.clone(), b.current()));
    assert_eq!(before.0, after.0);
    assert!(!after.0.contains_key("retries"));
    assert_eq!(before.1, after.1);
}

#[gpui::test]
fn backspace_and_delete_in_an_inspector_field_do_not_delete_the_selected_node(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    let ids = node_ids(&s, visual);
    focus_field(&s, visual, BoardField::Prompt("dev".into()));
    visual.simulate_keystrokes("backspace");
    visual.simulate_keystrokes("delete");
    assert_eq!(node_ids(&s, visual), ids);
    assert!(ids.contains(&"dev".to_string()));
}
