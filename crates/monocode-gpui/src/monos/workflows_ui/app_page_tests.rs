//! The global Workflows page: a Mono's overview has no Workflows tab, a review is approved from
//! the page, and "Ask Mono to build a workflow" lets the user pick a Mono, fills its composer and
//! never sends.
use super::actions::ReviewIntent;
use super::tests::{A, click, history, open_tab, put, route, saved, workflow};
use super::*;
use crate::monos::tests::{chat_rows, setup};
use gpui::{TestAppContext, VisualTestContext};
use serde_json::json;

#[gpui::test]
fn a_monos_overview_has_no_workflows_tab(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    s.window
        .update(cx, |shell, _, cx| {
            shell.workspace.right_panel_width = 520.;
            shell.workspace.right_panel_open = true;
            cx.notify();
        })
        .unwrap();
    cx.run_until_parked();
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-overview-Habits").is_some());
    assert!(visual.debug_bounds("mono-overview-Workflows").is_none());
}

#[gpui::test]
fn the_page_lists_the_workflows_of_the_app_under_its_own_title(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(visual.debug_bounds("workflows-view").is_some());
    assert!(visual.debug_bounds("mono-workflow-wf-aaaaaaaa").is_some());
    // No Mono is open behind the page: it does not depend on one.
    assert_eq!(s.manager.read_with(visual, |m, _| m.active.clone()), None);
}

#[gpui::test]
fn a_workflow_that_needs_review_is_approved_from_the_global_page(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(
        &s,
        vec![workflow(json!({"enabled": false, "needsReview": true}))],
    );
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    // The list marks it, and enabling it asks for the review first.
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(WfRoute::Workflow(A.into()), cx)
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("mono-workflow-needs-review").is_some());
    s.manager.update(visual, |m, cx| {
        m.workflow_toggle_clicked(A, false, true, true, cx)
    });
    visual.run_until_parked();
    assert_eq!(
        route(&s, visual),
        Some(WfRoute::Review(A.into(), ReviewIntent::Enable))
    );
    click(visual, "mono-workflow-approve");
    let stored = &saved(&s)[0];
    assert!(stored.enabled && !stored.needs_review);
    assert!(
        history(&s, A).is_empty(),
        "approving to enable runs nothing"
    );
}

fn draft(s: &crate::monos::tests::Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager.read_with(visual, |m, cx| {
        m.open_chat(&s.id)
            .map(|chat| chat.read(cx).input.read(cx).content.to_string())
    })
}

#[gpui::test]
fn ask_mono_offers_the_monos_and_defaults_to_the_one_open_last(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.ask_mono_default()),
        Some(s.id.clone())
    );
    click(visual, "mono-workflow-ask");
    assert!(s.manager.read_with(visual, |m, _| {
        m.picker.is_some() && m.wf.pick == Some(Pick::AskMono)
    }));
}

#[gpui::test]
fn ask_mono_opens_the_picked_mono_with_a_sentence_and_never_sends(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![workflow(json!({}))]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    let before = chat_rows(&s).len();
    click(visual, "mono-workflow-ask");
    let id = s.id.clone();
    s.manager.update(visual, |m, cx| {
        m.workflow_picked(&Pick::AskMono, id.clone(), cx)
    });
    visual.run_until_parked();
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.active.clone()),
        Some(s.id.clone()),
        "the Mono is open"
    );
    assert_eq!(
        draft(&s, visual).as_deref(),
        Some("Help me build a workflow that ")
    );
    assert_eq!(chat_rows(&s).len(), before, "nothing was sent");
    assert!(
        visual.debug_bounds("workflows-view").is_none(),
        "the page is left for the Mono"
    );
}
