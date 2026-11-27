//! The scheduling cases of `useMonoWorkflows.test.ts` over the global store (real files, the real
//! host, no provider): what a look starts, the waker, requests in the file, the startup sweep and
//! quitting. The file-level cases are in core (`tick_tests`).
use super::*;
use crate::monos::tests::{Setup, call, setup};
use crate::workflows::Workflows;
use gpui::{Entity, TestAppContext};
use monocode_core::mono::workflows::{
    engine::{mark_interrupted, start_run},
    model::{CallerKind, RunCaller, RunStatus, Workflow, WorkflowRun},
    models::Probe,
    ordered::Json,
    parse::parse_workflow,
    store::{MonoDir, load_runs, load_workflows, update_runs, update_workflows},
};
use monocode_core::provider::Kind;
use serde_json::{Value, json};

fn definition(id: &str, nodes: Vec<Value>, edges: Vec<Value>, extra: Value) -> Workflow {
    let mut def = json!({
        "id": id, "name": "Ship it", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": false, "createdAt": 0,
    });
    for (key, value) in extra.as_object().unwrap() {
        def[key] = value.clone();
    }
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

/// A manual trigger alone: runs to the end by itself.
fn quick(id: &str, extra: Value) -> Workflow {
    definition(
        id,
        vec![json!({"id": "start", "type": "manual"})],
        vec![],
        extra,
    )
}

fn with_project(mut extra: Value, project: Option<&str>) -> Value {
    extra["inputs"] = json!({"repo": {"type": "project"}});
    if let Some(project) = project {
        extra["inputValues"] = json!({"repo": project});
    }
    extra
}

fn session_steps() -> (Vec<Value>, Vec<Value>) {
    (
        vec![
            json!({"id": "start", "type": "manual"}),
            json!({"id": "dev", "type": "session", "project": "{{inputs.repo}}", "prompt": "go",
                "checkout": "root", "output": {}}),
        ],
        vec![json!({"from": "start", "to": "dev"})],
    )
}

/// A manual trigger and a session step that never gets a turn in a test: the run stays going
/// until it is stopped.
fn waiting(s: &Setup, id: &str, extra: Value) -> Workflow {
    let (nodes, edges) = session_steps();
    definition(id, nodes, edges, with_project(extra, Some(&s.project)))
}

/// A workflow that cannot start: its project input has no value.
fn broken(id: &str, extra: Value) -> Workflow {
    let (nodes, edges) = session_steps();
    definition(id, nodes, edges, with_project(extra, None))
}

fn workflows_of(s: &Setup, cx: &mut TestAppContext) -> Entity<Workflows> {
    s.window
        .update(cx, |shell, _, _| shell.workflows().expect("workflows"))
        .unwrap()
}

fn dir(s: &Setup) -> WorkflowsDir {
    s.workflows_dir()
}

fn put(s: &Setup, workflows: Vec<Workflow>) {
    update_workflows(&dir(s), |_| Ok((workflows.clone(), ()))).unwrap();
}

fn saved(s: &Setup) -> Vec<Workflow> {
    load_workflows(&dir(s)).unwrap().workflows
}

fn runs(s: &Setup, workflow: &str) -> Vec<WorkflowRun> {
    load_runs(&dir(s), workflow).unwrap()
}

fn put_runs(s: &Setup, workflow: &str, list: Vec<WorkflowRun>) {
    update_runs(&dir(s), workflow, |_| Ok((list.clone(), ()))).unwrap();
}

/// A look, as the clock or the waker makes it, with everything it starts run to the end.
fn look(s: &Setup, cx: &mut TestAppContext) {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.update(cx, |w, cx| w.check_workflows(cx)));
    cx.run_until_parked();
}

fn running(s: &Setup, workflow: &str, cx: &mut TestAppContext) -> bool {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.read(cx).is_running(workflow))
}

fn stop(s: &Setup, workflow: &str, cx: &mut TestAppContext) -> bool {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.update(cx, |w, _| w.stop_run(workflow)))
}

fn forget_startup(s: &Setup, cx: &mut TestAppContext) {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.update(cx, |w, _| w.checks.swept = false));
}

const A: &str = "wf-aaaaaaaa";
const B: &str = "wf-bbbbbbbb";

#[gpui::test]
fn a_requested_run_starts_on_the_look_and_its_summary_is_kept_on_the_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, RunStatus::Done);
    assert_eq!(
        history[0].caller,
        RunCaller::new(CallerKind::Manual, "start")
    );
    assert_eq!(
        history[0].summary.as_ref().map(|s| s.title.as_str()),
        Some("Ship it \u{b7} completed")
    );
    let after = &saved(&s)[0];
    assert!(!after.run_requested);
    assert_eq!(after.last_run_id.as_deref(), Some(history[0].id.as_str()));
    // A manual workflow stays paused; only the request ran it.
    assert!(!after.enabled);
    assert!(!running(&s, A, cx));
}

#[gpui::test]
fn a_run_a_mono_asked_for_records_it_as_the_caller(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut asked = quick(A, json!({"runRequested": true}));
    asked.run_caller = Some(RunCaller::mono("mono-7", "start"));
    put(&s, vec![asked]);
    look(&s, cx);
    assert_eq!(runs(&s, A)[0].caller, RunCaller::mono("mono-7", "start"));
}

#[gpui::test]
fn a_due_schedule_is_claimed_and_run_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    let schedule = json!({"id": "start", "type": "schedule", "schedule": {"scheduleKind": "daily", "time": "09:00", "minute": 0, "dayOfWeek": 1}});
    let due = definition(
        A,
        vec![schedule],
        vec![],
        json!({"enabled": true, "nextRunAt": monocode_core::time::now_ms() - 1_000}),
    );
    put(&s, vec![due]);
    look(&s, cx);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1, "the second look finds it already claimed");
    assert_eq!(
        history[0].caller,
        RunCaller::new(CallerKind::Schedule, "start")
    );
    assert_eq!(history[0].status, RunStatus::Done);
    assert!(saved(&s)[0].next_run_at.unwrap() > monocode_core::time::now_ms());
}

#[gpui::test]
fn a_workflow_that_cannot_start_gets_a_failed_run_with_its_message_on_the_run_page(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(&s, vec![broken(A, json!({"runRequested": true}))]);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].status, RunStatus::Failed);
    assert!(
        history[0]
            .error
            .as_deref()
            .unwrap()
            .starts_with("The workflow is not valid: ")
    );
    let summary = history[0].summary.as_ref().expect("a message on the run");
    assert_eq!(summary.title, "Ship it \u{b7} failed");
    assert!(
        summary.text.contains("The workflow could not start. "),
        "{}",
        summary.text
    );
    assert!(!running(&s, A, cx));
}

#[gpui::test]
fn the_waker_makes_a_request_act_at_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    // Written the way another window would write it; nothing looks until the waker rings.
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    cx.run_until_parked();
    assert!(runs(&s, A).is_empty());
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    signals.request_check_now();
    cx.run_until_parked();
    assert_eq!(runs(&s, A)[0].status, RunStatus::Done);
    assert!(!signals.take_check_now(), "the look answered the request");
}

#[gpui::test]
fn the_clock_looks_every_minute(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    cx.executor().advance_clock(Duration::from_secs(30));
    cx.run_until_parked();
    assert!(runs(&s, A).is_empty(), "not yet");
    cx.executor().advance_clock(Duration::from_secs(31));
    cx.run_until_parked();
    assert_eq!(runs(&s, A).len(), 1);
}

#[gpui::test]
fn a_second_run_of_a_workflow_waits_for_the_first(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(&s, A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    assert_eq!(runs(&s, A).len(), 1);
    // Asked again while it goes: the request waits, nothing else starts.
    update_workflows(&dir(&s), |mut list| {
        list[0].run_requested = true;
        Ok((list, ()))
    })
    .unwrap();
    look(&s, cx);
    assert_eq!(runs(&s, A).len(), 1);
    assert!(saved(&s)[0].run_requested);
    // Another workflow goes side by side.
    put(
        &s,
        vec![saved(&s).remove(0), quick(B, json!({"runRequested": true}))],
    );
    look(&s, cx);
    assert_eq!(runs(&s, B)[0].status, RunStatus::Done);
    assert!(running(&s, A, cx));
    // Once the first ends the waiting request runs.
    stop(&s, A, cx);
    cx.run_until_parked();
    assert_eq!(runs(&s, A)[0].status, RunStatus::Stopped);
    look(&s, cx);
    assert_eq!(runs(&s, A).len(), 2);
}

#[gpui::test]
fn a_stop_written_in_the_file_stops_the_run_that_is_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(&s, A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    update_workflows(&dir(&s), |mut list| {
        list[0].stop_requested = true;
        Ok((list, ()))
    })
    .unwrap();
    look(&s, cx);
    assert_eq!(runs(&s, A)[0].status, RunStatus::Stopped);
    assert!(!saved(&s)[0].stop_requested);
    assert!(!running(&s, A, cx));
    assert_eq!(
        runs(&s, A)[0].summary.as_ref().map(|s| s.title.as_str()),
        Some("Ship it \u{b7} stopped")
    );
}

#[gpui::test]
fn a_resume_written_in_the_file_continues_an_interrupted_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    let def = quick(A, json!({}));
    let interrupted = mark_interrupted(&start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-old",
    ));
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    put(&s, vec![quick(A, json!({"resumeRunId": "run-old"}))]);
    put_runs(&s, A, vec![interrupted]);
    look(&s, cx);
    let history = runs(&s, A);
    assert_eq!(history.len(), 1, "the same run continues");
    assert_eq!(history[0].id, "run-old");
    assert_eq!(history[0].status, RunStatus::Done);
    assert_eq!(saved(&s)[0].resume_run_id, None);
}

#[gpui::test]
fn the_first_look_interrupts_runs_a_previous_session_left_but_not_the_ones_it_drives(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(
        &s,
        vec![
            waiting(&s, A, json!({"runRequested": true})),
            quick(B, json!({})),
        ],
    );
    look(&s, cx);
    assert!(running(&s, A, cx));
    let live_id = runs(&s, A)[0].id.clone();
    // A run a previous session of the app left going.
    let mut stale = start_run(
        &quick(B, json!({})),
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-stale",
    );
    stale.status = RunStatus::Running;
    put_runs(&s, B, vec![stale]);
    forget_startup(&s, cx);
    look(&s, cx);
    assert_eq!(runs(&s, B)[0].status, RunStatus::Interrupted);
    let driven = runs(&s, A);
    assert_eq!(
        (driven[0].id.as_str(), driven[0].status),
        (live_id.as_str(), RunStatus::Running)
    );
    // It is done once: a later look does not sweep again.
    let mut newer = start_run(
        &quick(B, json!({})),
        RunCaller::new(CallerKind::Manual, "start"),
        2_000,
        "run-new",
    );
    newer.status = RunStatus::Running;
    put_runs(&s, B, vec![newer]);
    look(&s, cx);
    assert_eq!(runs(&s, B)[0].status, RunStatus::Running);
}

#[gpui::test]
fn what_an_old_per_mono_file_had_enabled_no_longer_fires_and_the_file_is_left_alone(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    // The file the Mono's workflows lived in before they were global: a run is asked for and a
    // schedule is overdue.
    let old = MonoDir::new(&s.root, &s.id);
    let due = definition(
        B,
        vec![
            json!({"id": "start", "type": "schedule", "schedule": {"scheduleKind": "daily", "time": "09:00", "minute": 0, "dayOfWeek": 1}}),
        ],
        vec![],
        json!({"enabled": true, "nextRunAt": 1}),
    );
    update_workflows(&old, |_| {
        Ok((
            vec![quick(A, json!({"runRequested": true})), due.clone()],
            (),
        ))
    })
    .unwrap();
    let before = load_workflows(&old).unwrap();
    look(&s, cx);
    forget_startup(&s, cx);
    look(&s, cx);
    cx.executor().advance_clock(Duration::from_secs(61));
    cx.run_until_parked();
    assert!(runs(&s, A).is_empty() && runs(&s, B).is_empty());
    assert!(saved(&s).is_empty(), "the global store is its own file");
    assert!(load_runs(&old, A).unwrap().is_empty());
    assert!(load_runs(&old, B).unwrap().is_empty());
    let after = load_workflows(&old).unwrap();
    assert_eq!(before.hash, after.hash, "never written");
    assert!(after.workflows[0].run_requested);
}

#[gpui::test]
fn a_look_asked_for_during_a_look_follows_it_and_never_overlaps(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    let workflows = workflows_of(&s, cx);
    cx.update(|cx| {
        workflows.update(cx, |w, cx| {
            w.check_workflows(cx);
            assert!(w.checks.checking);
            w.check_workflows(cx);
            assert!(w.checks.again);
        })
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let checks = &workflows.read(cx).checks;
        assert!(!checks.checking && !checks.again);
    });
    assert_eq!(runs(&s, A).len(), 1);
}

#[gpui::test]
fn quitting_leaves_a_run_running_in_its_file_and_the_next_start_interrupts_it(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    put(&s, vec![waiting(&s, A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    cx.update(begin_quit);
    cx.run_until_parked();
    assert!(!running(&s, A, cx));
    // No stop: the file says `running`, no summary was written.
    assert_eq!(runs(&s, A)[0].status, RunStatus::Running);
    assert_eq!(runs(&s, A)[0].summary, None);
    // The next start (another process: the free function stands in for its first look) marks it,
    // and it can be resumed.
    let signals = monocode_core::mono::workflows::cli::Signals::default();
    let looked = look_at(
        &dir(&s),
        &[],
        true,
        &signals,
        probes(probe_nothing),
        monocode_core::time::now_ms(),
    );
    assert!(looked.swept);
    assert_eq!(runs(&s, A)[0].status, RunStatus::Interrupted);
}

#[gpui::test]
fn the_quit_path_reaches_the_runner_through_the_app(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(&s, A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    cx.update(begin_quit);
    cx.run_until_parked();
    assert!(!running(&s, A, cx));
    assert_eq!(runs(&s, A)[0].status, RunStatus::Running);
}

#[gpui::test]
fn deleting_a_mono_leaves_the_workflow_runs_going(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![waiting(&s, A, json!({"runRequested": true}))]);
    look(&s, cx);
    assert!(running(&s, A, cx));
    let deleted = cx.update(|cx| s.manager.update(cx, |m, cx| m.mobile_delete(&s.id, cx)));
    assert_eq!(deleted, Ok(Value::Null));
    cx.run_until_parked();
    cx.update(|cx| assert!(s.manager.read(cx).roster.get(&s.id).is_none()));
    // No Mono owns the run, so it goes on.
    assert!(running(&s, A, cx));
    assert_eq!(runs(&s, A)[0].status, RunStatus::Running);
}

#[test]
fn a_look_over_a_missing_store_is_empty_and_counts_as_swept() {
    let root = std::env::temp_dir().join(format!("wf-look-{}", monocode_core::mono::new_id()));
    let dir = WorkflowsDir::new(root.join("workflows"));
    let signals = monocode_core::mono::workflows::cli::Signals::default();
    let looked = look_at(&dir, &[], true, &signals, probes(probe_nothing), 1);
    assert!(looked.swept && looked.found.is_none() && looked.errors.is_empty());
    let looked = look_at(&dir, &[], false, &signals, probes(probe_nothing), 1);
    assert!(!looked.swept);
}

#[gpui::test]
fn the_quit_does_not_need_the_entity_to_be_free(cx: &mut TestAppContext) {
    // The tray's Quit runs inside an update of the entities.
    let s = setup(cx);
    let workflows = workflows_of(&s, cx);
    cx.update(|cx| workflows.update(cx, |_, cx| begin_quit(cx)));
    cx.update(|cx| assert!(workflows.read(cx).runner.is_quitting()));
}

#[gpui::test]
fn after_the_quit_a_look_starts_nothing_and_writes_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    put(&s, vec![quick(A, json!({"runRequested": true}))]);
    let before = std::fs::read_to_string(dir(&s).root().join("workflows.json")).unwrap_or_default();
    cx.update(begin_quit);
    look(&s, cx);
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    signals.request_check_now();
    cx.executor().advance_clock(Duration::from_secs(61));
    cx.run_until_parked();
    assert!(runs(&s, A).is_empty());
    assert!(saved(&s)[0].run_requested, "the request was not claimed");
    let after = std::fs::read_to_string(dir(&s).root().join("workflows.json")).unwrap_or_default();
    assert_eq!(before, after);
}

#[gpui::test]
fn a_look_that_was_going_when_the_app_quit_starts_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    // One that would be failed and reported, and one that would run.
    put(
        &s,
        vec![
            quick(A, json!({"runRequested": true})),
            broken(B, json!({"runRequested": true})),
        ],
    );
    // The look is under way (its claim may already be written) when the quit begins.
    let workflows = workflows_of(&s, cx);
    cx.update(|cx| workflows.update(cx, |w, cx| w.check_workflows(cx)));
    cx.update(begin_quit);
    cx.run_until_parked();
    assert!(
        runs(&s, A).is_empty() && runs(&s, B).is_empty(),
        "no run was written"
    );
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(!monos.workflows.is_running(A) && !monos.workflow_signals.is_running(A));
    });
}

fn probes(models: fn(Kind) -> Probe) -> Probes {
    Probes {
        models,
        requirements: preflight_met,
    }
}

/// Nothing a workflow requires is ever missing.
fn preflight_met(_: &Workflow) -> monocode_core::mono::workflows::preflight::Preflight {
    Default::default()
}

fn probe_nothing(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: None,
    }
}

/// A look whose background half is done but whose runs are not started yet (what the model
/// probing of a slow catalog leaves open), as the entity would be in the middle of it.
fn half_a_look(s: &Setup, cx: &mut TestAppContext) -> Looked {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.update(cx, |w, _| w.checks.checking = true));
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    look_at(
        &dir(s),
        &[],
        false,
        &signals,
        probes(probe_nothing),
        monocode_core::time::now_ms(),
    )
}

#[gpui::test]
fn a_workflow_claimed_but_not_started_yet_counts_as_running(cx: &mut TestAppContext) {
    let s = setup(cx);
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    put(
        &s,
        vec![waiting(
            &s,
            A,
            json!({"runRequested": true, "enabled": true}),
        )],
    );
    let looked = half_a_look(&s, cx);
    assert!(signals.is_running(A), "marked at the claim");
    assert!(runs(&s, A).is_empty());
    let ask = |request: &str, action: &str, cx: &mut TestAppContext| {
        call(&s, request, action, json!({"id": A}), cx)
            .try_recv()
            .unwrap()
    };
    // It cannot be removed from under the run, run twice, or called not running.
    assert_eq!(
        ask("rm-1", "workflows.remove", cx).unwrap_err(),
        "Stop the workflow before removing it"
    );
    assert!(
        ask("run-1", "workflows.run", cx)
            .unwrap_err()
            .contains("already running")
    );
    ask("stop-1", "workflows.stop", cx).unwrap();
    // The look in progress ends: the run starts, and the stop asked for meanwhile follows it.
    let workflows = workflows_of(&s, cx);
    cx.update(|cx| workflows.update(cx, |w, cx| w.workflows_looked(looked, cx)));
    cx.run_until_parked();
    let history = runs(&s, A);
    assert_eq!(history.len(), 1, "one run, not two");
    assert_eq!(history[0].status, RunStatus::Stopped);
    assert!(!saved(&s).is_empty(), "still there");
    assert!(!saved(&s)[0].stop_requested);
    assert!(!signals.is_running(A));
}

#[gpui::test]
fn a_claim_that_is_not_started_gives_the_mark_back(cx: &mut TestAppContext) {
    let s = setup(cx);
    let signals = cx.update(|cx| s.manager.read(cx).workflow_signals.clone());
    put(&s, vec![waiting(&s, A, json!({"runRequested": true}))]);
    let looked = half_a_look(&s, cx);
    assert!(signals.is_running(A));
    // The app begins to quit before the look ends: nothing starts, and the mark goes.
    cx.update(begin_quit);
    let workflows = workflows_of(&s, cx);
    cx.update(|cx| workflows.update(cx, |w, cx| w.workflows_looked(looked, cx)));
    assert!(!signals.is_running(A));
    assert!(runs(&s, A).is_empty());
}
