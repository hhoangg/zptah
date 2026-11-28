//! The event seams of the `Workflows` entity: a run for an item goes through the claim and the
//! queue, a paused workflow never claims, a test run takes no claim, and a removed workflow
//! forgets its claims. Against the real entity over a test shell; no provider is ever started.
use super::*;
use crate::{
    monos::tests::{Setup, setup},
    runtime::locked,
};
use gpui::{Entity, TestAppContext};
use monocode_core::mono::workflows::{
    cli::WorkflowHost as _,
    event::event_key,
    global::WorkflowsDir,
    model::{CallerKind, RunStatus, Workflow},
    ordered::Json,
    parse::parse_workflow,
    store::{load_runs, update_workflows},
};
use serde_json::json;
use std::{cell::RefCell, rc::Rc};

const WF: &str = "wf-events01";

fn workflow(s: &Setup, extra: serde_json::Value) -> Workflow {
    let mut def = json!({
        "id": WF, "name": "On new PR", "inputs": {}, "inputValues": {},
        "nodes": [
            {"id": "start", "type": "event", "source": "github", "event": "pull_request_opened",
             "project": s.project},
            {"id": "dev", "type": "session", "project": s.project, "prompt": "Review it",
             "checkout": "root", "outputs": {}},
        ],
        "edges": [{"from": "start", "to": "dev"}],
        "workspace": {"mode": "none"},
        "enabled": true, "createdAt": 1,
    });
    for (key, value) in extra.as_object().unwrap() {
        def[key] = value.clone();
    }
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

fn save(s: &Setup, cx: &mut TestAppContext, w: Workflow) {
    let dir = workflows_dir(s, cx);
    update_workflows(&dir, |_| Ok((vec![w.clone()], ()))).unwrap();
}

fn workflows_dir(s: &Setup, cx: &mut TestAppContext) -> WorkflowsDir {
    let workflows = entity(s, cx);
    cx.update(|cx| workflows.read(cx).dir().clone())
}

fn entity(s: &Setup, cx: &mut TestAppContext) -> Entity<Workflows> {
    s.window
        .update(cx, |shell, _, _| shell.workflows().expect("workflows"))
        .unwrap()
}

fn item(s: &Setup, number: i64) -> monocode_core::inbox::Item {
    monocode_core::inbox::Item {
        provider: "github".into(),
        kind: "pr".into(),
        number,
        repo: "acme/app".into(),
        project_path: s.project.clone(),
        title: format!("Fix {number}"),
        url: format!("https://github.com/acme/app/pull/{number}"),
        ..Default::default()
    }
}

fn answer<T: 'static>(cx: &mut TestAppContext, task: Task<T>) -> T {
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    cx.spawn(async move |_| *out.borrow_mut() = Some(task.await))
        .detach();
    cx.run_until_parked();
    slot.take().expect("the start answered")
}

fn fire(s: &Setup, cx: &mut TestAppContext, number: i64) -> EventStarted {
    let workflows = entity(s, cx);
    let item = item(s, number);
    let task =
        cx.update(|cx| workflows.update(cx, |w, cx| w.start_event_run(WF, "start", item, cx)));
    answer(cx, task)
}

fn unclaimed(s: &Setup) -> usize {
    locked(&s.store)
        .unwrap()
        .unclaimed_workflow_claims()
        .unwrap()
        .len()
}

#[gpui::test]
fn an_item_starts_one_run_and_the_claim_names_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, workflow(&s, json!({})));
    let EventStarted::Started { run_id } = fire(&s, cx, 7) else {
        panic!("expected a started run")
    };
    let dir = workflows_dir(&s, cx);
    let runs = load_runs(&dir, WF).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, run_id);
    assert_eq!(runs[0].caller.kind, CallerKind::Event);
    assert_eq!(runs[0].caller.trigger_node, "start");
    assert!(runs[0].caller.item.is_some());
    // The claim has its run, so the startup check would leave it alone.
    assert_eq!(unclaimed(&s), 0);
    let workflows = entity(&s, cx);
    assert!(cx.update(|cx| workflows.read(cx).is_running(WF)));
}

#[gpui::test]
fn the_same_item_again_starts_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, workflow(&s, json!({})));
    assert!(matches!(fire(&s, cx, 7), EventStarted::Started { .. }));
    assert_eq!(fire(&s, cx, 7), EventStarted::Duplicate);
    let dir = workflows_dir(&s, cx);
    assert_eq!(load_runs(&dir, WF).unwrap().len(), 1);
}

#[gpui::test]
fn a_busy_workflow_queues_the_item(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, workflow(&s, json!({"concurrency": {"max": 1}})));
    assert!(matches!(fire(&s, cx, 1), EventStarted::Started { .. }));
    let queued = fire(&s, cx, 2);
    assert!(
        matches!(queued, EventStarted::Queued { position: 1, .. }),
        "{queued:?}"
    );
    let dir = workflows_dir(&s, cx);
    let runs = load_runs(&dir, WF).unwrap();
    assert_eq!(
        runs.iter()
            .filter(|r| r.status == RunStatus::Queued)
            .count(),
        1
    );
    assert_eq!(unclaimed(&s), 0);
}

#[gpui::test]
fn a_paused_workflow_holds_the_item_without_claiming_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, workflow(&s, json!({"enabled": false})));
    assert_eq!(
        fire(&s, cx, 7),
        EventStarted::Held(monocode_core::mono::workflows::event_run::Held::Paused)
    );
    let dir = workflows_dir(&s, cx);
    assert!(load_runs(&dir, WF).unwrap().is_empty());
    assert!(
        locked(&s.store)
            .unwrap()
            .claim_workflow_item(WF, "start", &event_key(&item(&s, 7)))
            .unwrap()
    );
}

#[gpui::test]
fn a_test_run_takes_no_claim_and_works_while_paused(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, workflow(&s, json!({"enabled": false})));
    let workflows = entity(&s, cx);
    let test_item = item(&s, 7);
    let task =
        cx.update(|cx| workflows.update(cx, |w, cx| w.start_test_run(WF, "start", test_item, cx)));
    let started = answer(cx, task).expect("allowed while paused");
    assert!(matches!(started, EventStarted::Started { .. }));
    let dir = workflows_dir(&s, cx);
    let runs = load_runs(&dir, WF).unwrap();
    assert_eq!(runs[0].caller.kind, CallerKind::Manual);
    assert!(runs[0].caller.item.is_some());
    assert!(
        locked(&s.store)
            .unwrap()
            .claim_workflow_item(WF, "start", &event_key(&item(&s, 7)))
            .unwrap(),
        "no claim was taken"
    );
}

#[gpui::test]
fn a_test_run_is_refused_while_the_workflow_needs_review(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = workflow(&s, json!({}));
    w.needs_review = true;
    save(&s, cx, w);
    let workflows = entity(&s, cx);
    let test_item = item(&s, 7);
    let task =
        cx.update(|cx| workflows.update(cx, |w, cx| w.start_test_run(WF, "start", test_item, cx)));
    let refused = answer(cx, task).unwrap_err();
    assert!(refused.contains("Approve"), "{refused}");
}

#[gpui::test]
fn removing_a_workflow_forgets_its_claims(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, workflow(&s, json!({})));
    assert!(matches!(fire(&s, cx, 7), EventStarted::Started { .. }));
    let workflows = entity(&s, cx);
    cx.update(|cx| {
        let mut host = workflows.read(cx).store_host(cx);
        host.clear_runs(WF);
    });
    assert!(
        locked(&s.store)
            .unwrap()
            .claim_workflow_item(WF, "start", &event_key(&item(&s, 7)))
            .unwrap()
    );
}

#[gpui::test]
fn the_startup_check_gives_back_a_claim_that_never_got_a_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    {
        let store = locked(&s.store).unwrap();
        assert!(
            store
                .claim_workflow_item(WF, "start", "github:pr:acme/app:7")
                .unwrap()
        );
        assert!(
            store
                .claim_workflow_item(WF, "start", "github:pr:acme/app:8")
                .unwrap()
        );
        store
            .set_workflow_claim_run(WF, "start", "github:pr:acme/app:8", "run-8")
            .unwrap();
    }
    assert_eq!(unclaimed(&s), 1);
    let workflows = entity(&s, cx);
    cx.update(|cx| workflows.read(cx).release_unclaimed_events());
    assert_eq!(unclaimed(&s), 0);
    let store = locked(&s.store).unwrap();
    assert!(
        store
            .claim_workflow_item(WF, "start", "github:pr:acme/app:7")
            .unwrap()
    );
    assert!(
        !store
            .claim_workflow_item(WF, "start", "github:pr:acme/app:8")
            .unwrap()
    );
}
