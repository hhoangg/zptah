//! Pages drawn again and again, the clock of a hidden view, and what a page leaves behind when
//! the view is left.
use super::board::BoardField;
use super::board_tests::{do_with, on_board, open_board, ticket};
use super::run_model_tests::{linear, nr, run_of};
use super::tests::{A, click, open_tab, put};
use super::*;
use crate::monos::tests::setup;
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::store::update_runs;
use serde_json::json;
use std::time::Instant;

#[gpui::test]
fn the_board_that_was_left_is_not_built_again_by_later_frames(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    click(visual, "mono-page-back");
    assert_eq!(s.manager.read_with(visual, |m, _| m.wf.route.clone()), None);
    // Drawing the page again must not make a board.
    for _ in 0..3 {
        frame(&s, visual);
    }
    assert!(s.manager.read_with(visual, |m, _| m.wf.board.is_none()));
    assert!(s.manager.read_with(visual, |m, _| {
        !m.fields.keys().any(|k| k.starts_with("wf-b:"))
    }));
}

#[gpui::test]
fn the_board_drops_its_field_subscriptions_with_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    // The form of a node has text fields, each with its subscriptions.
    do_with(&s, visual, |m, cx| {
        m.board_graph_event(&super::graph::GraphEvent::Click("fetch".into()), cx)
    });
    visual.refresh().unwrap();
    let held = s.manager.read_with(visual, |m, _| {
        m.wf.board.as_ref().map(|b| b.subscriptions())
    });
    assert!(held.is_some_and(|n| n > 0));
    click(visual, "mono-page-back");
    visual.run_until_parked();
    // The pages the board comes back to have made what they keep once; another visit to the
    // board adds nothing.
    let before = s.manager.read_with(visual, |m, _| m.observation_count());
    for _ in 0..2 {
        s.manager.update(visual, |m, cx| {
            m.open_workflow_route(WfRoute::Board(A.into()), cx)
        });
        visual.run_until_parked();
        click(visual, "mono-page-back");
        visual.run_until_parked();
    }
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.observation_count()),
        before
    );
}

/// One more frame of the page.
fn frame(s: &crate::monos::tests::Setup, visual: &mut VisualTestContext) {
    visual
        .update_window(s.window.into(), |_, w, cx| {
            w.simulate_next_frame(cx);
            w.draw(cx).clear(cx);
        })
        .unwrap();
    visual.run_until_parked();
}

fn two_runs(s: &crate::monos::tests::Setup) {
    let run = |id: &str| {
        let mut r = run_of(
            &linear(),
            "done",
            json!({}),
            json!([nr("start", "", "done", json!({}))]),
        );
        r.id = id.into();
        r
    };
    update_runs(&s.workflows_dir(), A, |_| {
        Ok((vec![run("run-2"), run("run-1")], ()))
    })
    .unwrap();
}

#[gpui::test]
fn opening_a_second_run_page_after_the_first_keeps_one_canvas(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![ticket_free()]);
    two_runs(&s);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(WfRoute::Run(A.into(), "run-1".into()), cx)
    });
    visual.run_until_parked();
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(WfRoute::Run(A.into(), "run-2".into()), cx)
    });
    visual.run_until_parked();
    // The page is drawn for a while: the state stays the new run's.
    let opens = s.manager.read_with(visual, |m, _| m.wf.run_opens);
    for _ in 0..4 {
        frame(&s, visual);
    }
    // One build for each run page opened, however many frames drew the page.
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.wf.run_opens),
        opens,
        "drawing the run page rebuilt it"
    );
    assert_eq!(opens, 2, "each run page opened built its state once");
    let key = s.manager.read_with(visual, |m, _| m.wf.run.key.clone());
    assert_eq!(key, Some((A.to_string(), "run-2".to_string())));
}

fn ticket_free() -> Workflow {
    super::tests::workflow(json!({}))
}

#[gpui::test]
fn the_clock_stops_when_the_view_is_not_drawn_and_starts_again_when_it_is(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![ticket_free()]);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.wf.poll.is_some()));
    // Drawn a moment ago: the clock goes on.
    do_with_manager(&s, visual, |m, cx| assert!(m.workflows_poll(cx)));
    // Not drawn for a long while (the view is hidden): it stops and reads nothing.
    do_with_manager(&s, visual, |m, cx| {
        m.wf.last_drawn = Some(Instant::now() - std::time::Duration::from_secs(30));
        assert!(!m.workflows_poll(cx));
        assert!(m.wf.poll.is_none());
    });
    // Drawn again: it reads at once and the clock is back.
    put(
        &s,
        vec![
            ticket_free(),
            super::tests::workflow(json!({"id": "wf-bbbbbbbb"})),
        ],
    );
    s.manager.update(visual, |_, cx| cx.notify());
    visual.refresh().unwrap();
    visual.run_until_parked();
    assert!(s.manager.read_with(visual, |m, _| m.wf.poll.is_some()));
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_count()),
        Some(2)
    );
}

fn do_with_manager(
    s: &crate::monos::tests::Setup,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut Monos, &mut Context<Monos>),
) {
    s.manager.update(visual, f);
    visual.run_until_parked();
}

#[gpui::test]
fn a_board_does_not_ask_about_an_edit_while_its_own_save_is_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    super::board_tests::rename(&s, visual, "Mine");
    put(&s, vec![ticket(json!({"name": "By the Mono"}))]);
    do_with(&s, visual, |m, cx| {
        m.refresh_workflows(cx);
        if let Some(b) = m.wf.board.as_mut() {
            b.saving = true;
        }
    });
    assert!(on_board(&s, visual, |b| b.elsewhere.is_none()));
    // Once the save is over the read is news again.
    do_with(&s, visual, |m, cx| {
        if let Some(b) = m.wf.board.as_mut() {
            b.saving = false;
        }
        m.board_sync(cx);
    });
    assert!(on_board(&s, visual, |b| b.elsewhere.is_some()));
    let _ = BoardField::Name;
}

#[gpui::test]
fn opening_a_mono_closes_the_view_and_leaves_no_board_or_notice_behind(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| {
        m.workflow_said(Some("old".into()), true, cx)
    });
    let handle: gpui::AnyWindowHandle = s.window.into();
    let project = s.project.clone();
    handle
        .update(visual, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.create(vec![project], w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    let first = s.id.clone();
    handle
        .update(visual, |_, w, cx| {
            s.manager.update(cx, |m, cx| m.open(&first, w, cx))
        })
        .unwrap();
    visual.run_until_parked();
    s.manager.read_with(visual, |m, _| {
        assert!(m.wf.board.is_none());
        assert_eq!(m.wf.page_notice, None);
    });
}
