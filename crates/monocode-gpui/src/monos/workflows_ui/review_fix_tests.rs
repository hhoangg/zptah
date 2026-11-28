//! What the review of the workspace and concurrency work asked for: which run Stop ends, what
//! removing a workflow says and cancels, the runs list of a queue, the warnings that never block,
//! and the workspace and concurrency in the Settings mode.
use super::ask_dialogs::{RemoveAsk, remove_lines, run_label};
use super::board::{BoardField, BoardPick};
use super::board_tests::{do_with, on_board, open_board, pick, stored_first, ticket, type_in};
use super::layout::WfMode;
use super::run_model_tests::{linear, nr, run_of};
use super::tests::{A, click, history, open_tab, put, saved, settle};
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    model::{CallerKind, RunCaller, RunStatus, RunWorktree, WorkflowRun},
    queue::queued_run,
    store::{update_runs, update_workflows},
};
use serde_json::json;

fn put_runs(s: &Setup, runs: &[WorkflowRun]) {
    update_runs(&s.workflows_dir(), A, |_| Ok((runs.to_vec(), ()))).unwrap();
}

fn queued(w: &Workflow, id: &str, number: u64) -> WorkflowRun {
    let mut run = queued_run(
        w,
        RunCaller::new(CallerKind::Manual, "start"),
        number,
        id,
        number * 10,
    );
    run.workflow_id = A.into();
    run
}

fn tree(path: &str, branch: &str, removed: Option<u64>) -> RunWorktree {
    RunWorktree {
        path: path.into(),
        branch: branch.into(),
        base: "abc".into(),
        removed_at: removed,
    }
}

fn finished_with_tree(w: &Workflow, id: &str, worktree: RunWorktree) -> WorkflowRun {
    let mut run = run_of(
        w,
        "done",
        json!({}),
        json!([nr("start", "", "done", json!({}))]),
    );
    run.id = id.into();
    run.worktree = Some(worktree);
    run
}

fn visual_of(s: &Setup, cx: &mut TestAppContext) -> VisualTestContext {
    let visual = VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    visual
}

fn shown(visual: &mut VisualTestContext, selector: &'static str) -> bool {
    visual.debug_bounds(selector).is_some()
}

// ---- Stop asks which run -----------------------------------------------------------------

/// Two runs of the waiting workflow going at once.
fn two_going(s: &Setup, cx: &mut TestAppContext) {
    put(
        s,
        vec![super::tests::waiting_workflow(
            s,
            json!({"runRequested": true, "concurrency": {"max": 2}}),
        )],
    );
    crate::monos::tests::check_workflows(s, cx);
    cx.run_until_parked();
    update_workflows(&s.workflows_dir(), |mut list| {
        list[0].run_requested = true;
        Ok((list, ()))
    })
    .unwrap();
    crate::monos::tests::check_workflows(s, cx);
    cx.run_until_parked();
}

#[gpui::test]
fn stop_asks_which_run_when_several_go_and_stops_only_that_one(cx: &mut TestAppContext) {
    let s = setup(cx);
    two_going(&s, cx);
    assert_eq!(s.manager.read_with(cx, |m, _| m.workflows.live_count(A)), 2);
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    click(visual, "mono-workflow-stop-wf-aaaaaaaa");
    // Nothing stopped yet: it asks.
    assert!(shown(visual, "wf-stop-ask"));
    assert_eq!(
        s.manager
            .read_with(visual, |m, _| m.workflows.live_count(A)),
        2
    );
    // Cancel leaves both going.
    click(visual, "wf-stop-cancel");
    assert!(!shown(visual, "wf-stop-ask"));
    assert_eq!(
        s.manager
            .read_with(visual, |m, _| m.workflows.live_count(A)),
        2
    );
    // Picking a run stops that run only.
    click(visual, "mono-workflow-stop-wf-aaaaaaaa");
    let first = s.manager.read_with(visual, |m, _| {
        m.wf.stop_ask.as_ref().expect("asked").runs[0].0.clone()
    });
    let selector: &'static str = Box::leak(format!("wf-stop-run-{first}").into_boxed_str());
    click(visual, selector);
    assert!(!shown(visual, "wf-stop-ask"));
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, _| m.workflows.live_count(A)),
        1
    );
    assert!(
        !s.manager
            .read_with(visual, |m, _| m.workflows.is_run_live(&first))
    );
    // With one left there is nothing to ask: it stops at once.
    click(visual, "mono-workflow-stop-wf-aaaaaaaa");
    visual.run_until_parked();
    assert!(!shown(visual, "wf-stop-ask"));
    assert_eq!(
        s.manager
            .read_with(visual, |m, _| m.workflows.live_count(A)),
        0
    );
}

#[gpui::test]
fn stop_all_after_the_question_ends_every_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    two_going(&s, cx);
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    click(visual, "mono-workflow-stop-wf-aaaaaaaa");
    click(visual, "wf-stop-all");
    visual.run_until_parked();
    assert_eq!(
        s.manager
            .read_with(visual, |m, _| m.workflows.live_count(A)),
        0
    );
}

#[test]
fn the_stop_dialog_names_each_run_by_its_number_and_age() {
    let label = run_label(Some(3), 1_000, 61_000);
    assert!(label.starts_with("Run #3"), "{label}");
    assert!(run_label(None, 1_000, 61_000).starts_with("Run \u{b7}"));
}

// ---- removing a workflow --------------------------------------------------------------------

#[gpui::test]
fn removing_a_workflow_with_waiting_runs_and_worktrees_asks_then_cancels_and_removes(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut w = linear();
    w.last_run_id = Some("run-0".into());
    put(&s, vec![w.clone()]);
    put_runs(
        &s,
        &[
            queued(&w, "run-2", 2),
            queued(&w, "run-1", 1),
            finished_with_tree(
                &w,
                "run-0",
                tree("/code/app-worktrees/wf-x-1", "wf/x-1", None),
            ),
        ],
    );
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    click(visual, "mono-workflow-delete-wf-aaaaaaaa");
    settle(&s, visual);
    // Asked, and nothing happened yet.
    assert!(shown(visual, "wf-remove-ask"));
    let lines = s.manager.read_with(visual, |m, _| {
        remove_lines(m.wf.remove_ask.as_ref().expect("asked"))
    });
    assert!(
        lines[2].contains("2 queued runs will be cancelled"),
        "{lines:?}"
    );
    assert!(
        lines[3].contains("1 worktree") && lines[3].contains("wf/x-1"),
        "{lines:?}"
    );
    assert_eq!(saved(&s).len(), 1);
    // Cancel keeps everything.
    click(visual, "wf-remove-cancel");
    assert!(!shown(visual, "wf-remove-ask"));
    assert_eq!(saved(&s).len(), 1);
    assert_eq!(history(&s, A).len(), 3);
    // Confirming cancels the waiting runs, then removes the workflow and its history.
    click(visual, "mono-workflow-delete-wf-aaaaaaaa");
    settle(&s, visual);
    click(visual, "wf-remove-confirm");
    visual.run_until_parked();
    assert!(saved(&s).is_empty());
    assert!(history(&s, A).is_empty());
}

#[test]
fn the_removal_dialog_says_only_what_applies() {
    let ask = |queued: usize, trees: usize| RemoveAsk {
        workflow: A.into(),
        name: "x".into(),
        page: true,
        queued: (0..queued).map(|n| format!("q{n}")).collect(),
        trees: (0..trees)
            .map(|n| (format!("/p{n}"), format!("wf/b{n}")))
            .collect(),
        requests: 0,
    };
    // Always the two lines of what goes and what stays; the rest only when it applies.
    assert_eq!(remove_lines(&ask(0, 0)).len(), 2);
    assert_eq!(remove_lines(&ask(1, 0)).len(), 3);
    assert!(
        remove_lines(&ask(1, 0))[2].starts_with("1 queued run will be cancelled; it never starts")
    );
    assert_eq!(remove_lines(&ask(0, 2)).len(), 3);
    assert!(remove_lines(&ask(0, 2))[2].contains("2 worktrees"));
    let mut with_requests = ask(0, 0);
    with_requests.requests = 2;
    assert!(remove_lines(&with_requests)[0].contains("2 runs asked for"));
}

#[gpui::test]
fn a_workflow_with_nothing_left_over_is_still_asked_about_and_goes_on_confirm(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let w = linear();
    put(&s, vec![w.clone()]);
    put_runs(
        &s,
        &[finished_with_tree(
            &w,
            "run-0",
            tree("/code/app-worktrees/wf-x-1", "wf/x-1", Some(9)),
        )],
    );
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    click(visual, "mono-workflow-delete-wf-aaaaaaaa");
    settle(&s, visual);
    // Always asked (D52), and the dialog says what goes and what stays.
    assert!(shown(visual, "wf-remove-ask"));
    assert_eq!(saved(&s).len(), 1);
    let lines = s.manager.read_with(visual, |m, _| {
        remove_lines(m.wf.remove_ask.as_ref().expect("asked"))
    });
    assert!(
        lines[0].starts_with("Goes:") && lines[1].starts_with("Stays:"),
        "{lines:?}"
    );
    click(visual, "wf-remove-confirm");
    visual.run_until_parked();
    assert!(saved(&s).is_empty());
}

// ---- the runs list of a queue ----------------------------------------------------------------

fn open_runs(s: &Setup, visual: &mut VisualTestContext, run: &str) {
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(WfRoute::Run(A.into(), run.into()), cx)
    });
    visual.run_until_parked();
    settle(s, visual);
}

#[gpui::test]
fn the_runs_list_shows_the_queue_and_each_waiting_run_cancels_on_its_own(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = linear();
    w.last_run_id = Some("run-0".into());
    put(&s, vec![w.clone()]);
    put_runs(&s, &[queued(&w, "run-2", 2), queued(&w, "run-1", 1)]);
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    open_runs(&s, visual, "run-1");
    // "0 running / 2 queued" in the runs list and in the header.
    assert!(shown(visual, "wf-runs-slots"));
    assert!(shown(visual, "wf-slots"));
    // Each waiting row says its place and has a Cancel of its own.
    assert!(shown(visual, "wf-run-row-detail-run-1"));
    assert!(shown(visual, "wf-run-row-cancel-run-2"));
    click(visual, "wf-run-row-cancel-run-2");
    visual.run_until_parked();
    let runs = history(&s, A);
    let status = |id: &str| runs.iter().find(|r| r.id == id).unwrap().status;
    assert_eq!(status("run-2"), RunStatus::Stopped);
    assert_eq!(status("run-1"), RunStatus::Queued);
}

#[gpui::test]
fn the_runs_list_can_cancel_every_waiting_run_at_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = linear();
    w.last_run_id = Some("run-0".into());
    put(&s, vec![w.clone()]);
    put_runs(
        &s,
        &[
            queued(&w, "run-3", 3),
            queued(&w, "run-2", 2),
            queued(&w, "run-1", 1),
        ],
    );
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    open_runs(&s, visual, "run-1");
    click(visual, "wf-runs-cancel-queued");
    click(visual, "wf-cancelall-confirm");
    visual.run_until_parked();
    assert!(
        history(&s, A)
            .iter()
            .all(|r| r.status == RunStatus::Stopped)
    );
}

// ---- warnings never block ----------------------------------------------------------------------

/// A workflow with a writer that goes into the project folder, with three runs at once.
fn risky() -> Workflow {
    ticket(json!({"workspace": {"mode": "none", "project": "{{inputs.project}}"}}))
}

#[gpui::test]
fn the_problems_bar_shows_warnings_as_advice_and_the_workflow_still_saves(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &risky(), cx);
    let visual = &mut visual;
    let (problems, warnings) = on_board(&s, visual, |b| (b.issues.len(), b.warnings.len()));
    assert_eq!(problems, 0, "advice is not a problem");
    assert!(warnings >= 1);
    assert!(shown(visual, "wf-board-issues"));
    // Not blocking: the draft can be saved and a paused save is not offered for it.
    do_with(&s, visual, |m, cx| {
        m.board_meta_edit(|meta| meta.name = "Renamed".into(), cx)
    });
    assert!(on_board(&s, visual, |b| b.can_save()));
    assert!(!shown(visual, "wf-board-issues-paused"));
    click(visual, "wf-board-issues-toggle");
    assert!(shown(visual, "wf-warning-0"));
    assert!(!shown(visual, "wf-problem-0"));
    click(visual, "wf-board-save");
    visual.run_until_parked();
    assert_eq!(stored_first(&s).name, "Renamed");
}

#[gpui::test]
fn the_review_page_lists_the_warnings_of_the_workflow(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = risky();
    w.needs_review = true;
    w.enabled = false;
    put(&s, vec![w.clone()]);
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    s.manager.update(visual, |m, cx| {
        m.open_workflow_review(A, super::actions::ReviewIntent::Enable, None, cx)
    });
    visual.run_until_parked();
    settle(&s, visual);
    assert!(shown(visual, "mono-workflow-warnings"));
    assert!(!shown(visual, "mono-workflow-problems"));
}

#[gpui::test]
fn a_project_the_app_does_not_have_is_a_warning_on_the_board(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = ticket(json!({}));
    w.inputs.insert(
        "other".to_string(),
        monocode_core::mono::workflows::model::WorkflowInput::new(
            monocode_core::mono::workflows::model::InputType::Project,
        ),
    );
    let mut visual = open_board(&s, &w, cx);
    let visual = &mut visual;
    // The app's projects are known: /work/api is one, the value of "other" is not.
    do_with(&s, visual, |m, cx| {
        m.wf.scope.projects = vec!["/work/api".to_string()];
        m.board_meta_edit(
            |meta| {
                meta.input_values.insert(
                    "other".to_string(),
                    monocode_core::mono::workflows::model::InputValue::String("/work/gone".into()),
                );
                meta.workspace.project = "{{inputs.other}}".into();
            },
            cx,
        );
    });
    let warnings = on_board(&s, visual, |b| {
        b.warnings
            .iter()
            .map(|i| i.message.clone())
            .collect::<Vec<_>>()
    });
    assert!(
        warnings
            .iter()
            .any(|m| m.contains("not one of the app's projects")),
        "{warnings:?}"
    );
    // Advice only: the draft is not blocked by it.
    assert!(on_board(&s, visual, |b| b.issues.is_empty()));
}

// ---- workspace and concurrency in Settings -------------------------------------------------------

#[gpui::test]
fn settings_edits_the_workspace_and_the_concurrency_and_saves_them(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut visual = open_board(&s, &ticket(json!({})), cx);
    let visual = &mut visual;
    do_with(&s, visual, |m, cx| m.wf_set_mode(WfMode::Settings, cx));
    settle(&s, visual);
    assert!(shown(visual, "wf-workspace"));
    // Its own worktree per run, from a project input, branching from another base branch.
    pick(&s, visual, BoardPick::WorkspaceMode, "perRun");
    pick(
        &s,
        visual,
        BoardPick::WorkspaceProject,
        "{{inputs.project}}",
    );
    settle(&s, visual);
    type_in(&s, visual, BoardField::BaseBranch, "develop");
    pick(&s, visual, BoardPick::Concurrency, "2");
    let meta = on_board(&s, visual, |b| b.meta.clone());
    assert_eq!(meta.workspace.project, "{{inputs.project}}");
    assert_eq!(meta.workspace.base_branch.as_deref(), Some("develop"));
    assert_eq!(meta.concurrency.max, 2.);
    assert!(on_board(&s, visual, |b| b.can_save()));
    click(visual, "wf-board-save");
    visual.run_until_parked();
    let stored = stored_first(&s);
    assert_eq!(stored.workspace.project, "{{inputs.project}}");
    assert_eq!(stored.workspace.base_branch.as_deref(), Some("develop"));
    assert_eq!(stored.concurrency.max, 2.);
    // The project folder instead: no worktree, so no base branch row.
    pick(&s, visual, BoardPick::WorkspaceMode, "none");
    settle(&s, visual);
    let meta = on_board(&s, visual, |b| b.meta.clone());
    assert_eq!(
        meta.workspace.mode,
        monocode_core::mono::workflows::model::WorkspaceMode::None
    );
    // A project of the app picked by path.
    pick(&s, visual, BoardPick::WorkspaceProject, "/work/api");
    assert_eq!(
        on_board(&s, visual, |b| b.meta.workspace.project.clone()),
        "/work/api"
    );
    // Back to no project, and the default of three runs.
    pick(&s, visual, BoardPick::WorkspaceProject, "");
    pick(&s, visual, BoardPick::Concurrency, "3");
    assert_eq!(on_board(&s, visual, |b| b.meta.concurrency.max), 3.);
}

// ---- the re-check ---------------------------------------------------------------------------------

#[gpui::test]
fn removing_a_workflow_that_is_going_keeps_its_waiting_runs(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = linear();
    w.last_run_id = Some("run-0".into());
    put(&s, vec![w.clone()]);
    put_runs(&s, &[queued(&w, "run-1", 1)]);
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    // A run is going (claimed) when the removal is confirmed.
    let signals = s
        .manager
        .read_with(visual, |m, _| m.workflow_signals.clone());
    click(visual, "mono-workflow-delete-wf-aaaaaaaa");
    settle(&s, visual);
    signals.claim_run(A, "run-9");
    click(visual, "wf-remove-confirm");
    visual.run_until_parked();
    // Refused before anything was cancelled: the workflow stays and so does its queue.
    assert_eq!(saved(&s).len(), 1);
    assert_eq!(history(&s, A)[0].status, RunStatus::Queued);
    signals.release_run(A, "run-9");
}

#[gpui::test]
fn a_run_asked_for_by_the_user_goes_behind_a_pending_mono_request(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::cli::{Signals, StoreHost};
    let s = setup(cx);
    let mut w = super::tests::workflow_in(&s, json!({}));
    w.run_requested = true;
    w.run_caller = Some(RunCaller::mono("mono-1", "start"));
    put(&s, vec![w]);
    let mut host = StoreHost::new(s.workflows_dir(), std::sync::Arc::new(Signals::default()));
    assert_eq!(super::actions::run_now(&mut host, A, None), None);
    let stored = saved(&s).remove(0);
    assert_eq!(
        stored.run_caller,
        Some(RunCaller::mono("mono-1", "start")),
        "the Mono's request was not overwritten"
    );
    assert_eq!(stored.run_backlog.len(), 1);
    assert!(stored.run_backlog[0].caller.is_none());
}

#[gpui::test]
fn cancel_all_queued_asks_first(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = linear();
    w.last_run_id = Some("run-0".into());
    put(&s, vec![w.clone()]);
    put_runs(&s, &[queued(&w, "run-2", 2), queued(&w, "run-1", 1)]);
    open_tab(&s, cx);
    let visual = &mut visual_of(&s, cx);
    open_runs(&s, visual, "run-1");
    click(visual, "wf-runs-cancel-queued");
    assert!(shown(visual, "wf-cancelall-ask"));
    assert!(history(&s, A).iter().all(|r| r.status == RunStatus::Queued));
    click(visual, "wf-cancelall-cancel");
    assert!(!shown(visual, "wf-cancelall-ask"));
    assert!(history(&s, A).iter().all(|r| r.status == RunStatus::Queued));
}
