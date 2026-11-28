//! The inspector's "Available variables" list.
use super::board::BoardField;
use super::board_tests::*;
use super::graph::GraphEvent;
use super::tests::click;
use crate::monos::tests::setup;
use gpui::TestAppContext;
use monocode_core::mono::workflows::model::NodeKind;
use serde_json::json;

fn prompt_of(node: monocode_core::mono::workflows::model::Node) -> String {
    let NodeKind::Session { prompt, .. } = node.kind else {
        panic!()
    };
    prompt
}

#[gpui::test]
fn a_step_lists_its_variables_by_group_and_leaves_out_downstream_steps(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    for selector in [
        "wf-variables",
        "wf-variables-group:trigger",
        "wf-variables-group:inputs",
        "wf-variables-group:run",
        "wf-variables-group:item",
        // The first step is upstream of the loop body; the next one is the previous round.
        "wf-variables-group:step:fetch",
        "wf-variables-group:step:review",
        "wf-var:{{inputs.project}}",
        "wf-var:{{run.round}}",
        "wf-var:{{item}}",
        "wf-var:{{fetch.tickets}}",
    ] {
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    // The first step has nothing upstream: no step group, and no output of a later step.
    event(&s, visual, GraphEvent::Click("fetch".into()));
    assert!(visual.debug_bounds("wf-variables-group:inputs").is_some());
    for selector in [
        "wf-variables-group:step:dev",
        "wf-variables-group:step:review",
        "wf-variables-group:step:report",
        "wf-variables-group:item",
        "wf-var:{{dev.pr_url}}",
    ] {
        assert!(visual.debug_bounds(selector).is_none(), "{selector}");
    }
}

#[gpui::test]
fn clicking_a_variable_inserts_it_at_the_caret_of_the_prompt(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("review".into()));
    type_in(
        &s,
        visual,
        BoardField::Prompt("review".into()),
        "Check  now",
    );
    do_with(&s, visual, |m, cx| {
        let field = m.fields[&BoardField::Prompt("review".into()).key()].clone();
        field.update(cx, |f, cx| f.select_range(6..6, cx));
    });
    // The list is the last part of a long form: scroll it into view.
    let inspector = visual.debug_bounds("wf-inspector").unwrap();
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: inspector.center(),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-4000.))),
        ..Default::default()
    });
    visual.run_until_parked();
    visual.refresh().unwrap();
    click(visual, "wf-var:{{dev.pr_url}}");
    assert_eq!(
        prompt_of(node_of(&s, visual, "review")),
        "Check {{dev.pr_url}} now"
    );
    // A second click goes right after the first insert.
    click(visual, "wf-var:{{inputs.project}}");
    assert_eq!(
        prompt_of(node_of(&s, visual, "review")),
        "Check {{dev.pr_url}}{{inputs.project}} now"
    );
}

#[gpui::test]
fn a_command_step_lists_variables_and_inserts_into_its_command(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("report".into()));
    type_in(&s, visual, BoardField::Command("report".into()), "echo ");
    assert!(
        visual
            .debug_bounds("wf-variables-group:step:each")
            .is_some()
    );
    click(visual, "wf-var:{{each.results}}");
    let NodeKind::Command { command, .. } = node_of(&s, visual, "report").kind else {
        panic!()
    };
    assert_eq!(command, "echo {{each.results}}");
}
