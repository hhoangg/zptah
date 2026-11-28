//! What an event run asks the host for: the access of each session, the node's folder, the work
//! item to link, and the trigger item in the prompt. Against the fake host.
use super::*;
use monocode_core::mono::workflows::{engine::start_run, model::CallerKind};

fn event_trigger() -> Value {
    json!({"id": "start", "type": "event", "source": "github", "event": "pull_request_opened",
           "project": "{{inputs.repo}}"})
}

fn item() -> Json {
    Json::from(&json!({
        "source": "github", "kind": "pull request", "ref": "#7", "title": "Fix the thing",
        "url": "https://github.com/acme/app/pull/7", "repo": "acme/app",
        "workItem": {"kind": "pr", "repo": "acme/app", "number": 7,
                     "url": "https://github.com/acme/app/pull/7"},
    }))
}

fn event_run(def: &Workflow) -> WorkflowRun {
    let mut caller = RunCaller::new(CallerKind::Event, "start");
    caller.item = Some(item());
    start_run(def, caller, 1_000, "run-1")
}

fn one_session(extra: Value) -> Workflow {
    workflow(
        vec![event_trigger(), session("dev", json!({}), extra)],
        vec![edge("start", "dev")],
    )
}

fn start_of(cx: &mut TestAppContext, def: &Workflow, run: WorkflowRun) -> SessionRequest {
    let rig = Rig::new(&["ok"]);
    assert!(rig.start_run(cx, run));
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done, "{def:?}");
    let w = rig.fake.world();
    w.starts[0].clone()
}

#[gpui::test]
fn a_session_defaults_to_auto_accept_edits_and_never_to_full_access(cx: &mut TestAppContext) {
    let def = one_session(json!({}));
    let request = start_of(cx, &def, event_run(&def));
    assert_eq!(request.access, "auto-accept-edits");
    // A manual run of a workflow with no event trigger gets the same default.
    let manual = simple(json!({}));
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &manual);
    cx.run_until_parked();
    assert_eq!(rig.fake.world().starts[0].access, "auto-accept-edits");
}

#[gpui::test]
fn the_access_a_node_names_reaches_the_session_request(cx: &mut TestAppContext) {
    for id in ["supervised", "auto-accept-edits", "auto", "full-access"] {
        let def = one_session(json!({"access": id}));
        let request = start_of(cx, &def, event_run(&def));
        assert_eq!(request.access, id);
    }
}

#[gpui::test]
fn every_session_node_gets_its_own_access(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            event_trigger(),
            session("dev", json!({}), json!({"access": "full-access"})),
            session("docs", json!({}), json!({})),
        ],
        vec![edge("start", "dev"), edge("dev", "docs")],
    );
    let rig = Rig::new(&["ok", "ok"]);
    assert!(rig.start_run(cx, event_run(&def)));
    cx.run_until_parked();
    let w = rig.fake.world();
    let access: Vec<_> = w.starts.iter().map(|s| s.access.as_str()).collect();
    assert_eq!(access, ["full-access", "auto-accept-edits"]);
}

#[gpui::test]
fn the_nodes_folder_is_passed_beside_the_workflows_folder(cx: &mut TestAppContext) {
    let def = one_session(json!({"folder": "fld-9"}));
    let request = start_of(cx, &def, event_run(&def));
    // The host files the session in the node's folder, and in the workflow's when it is gone.
    assert_eq!(request.folder_id.as_deref(), Some("fld-9"));
    assert_eq!(request.folder_name.as_deref(), Some("Ship it"));
    let plain = one_session(json!({}));
    assert_eq!(start_of(cx, &plain, event_run(&plain)).folder_id, None);
}

#[gpui::test]
fn link_work_item_hands_the_trigger_items_work_item_to_the_session(cx: &mut TestAppContext) {
    let def = one_session(json!({"linkWorkItem": true}));
    let request = start_of(cx, &def, event_run(&def));
    let linked = request.work_item.expect("a work item to link");
    assert_eq!(linked.kind, "pr");
    assert_eq!(linked.repo, "acme/app");
    assert_eq!(linked.number, 7);
    assert_eq!(linked.url, "https://github.com/acme/app/pull/7");
}

#[gpui::test]
fn without_link_work_item_or_without_an_item_nothing_is_linked(cx: &mut TestAppContext) {
    let off = one_session(json!({}));
    assert_eq!(start_of(cx, &off, event_run(&off)).work_item, None);
    let explicit_off = one_session(json!({"linkWorkItem": false}));
    assert_eq!(
        start_of(cx, &explicit_off, event_run(&explicit_off)).work_item,
        None
    );
    // A run with no item (started by hand) has nothing to link.
    let on = one_session(json!({"linkWorkItem": true}));
    let manual = start_run(
        &on,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    assert_eq!(start_of(cx, &on, manual).work_item, None);
}

#[gpui::test]
fn the_prompt_reads_the_trigger_item_and_the_trigger_node_has_it_as_output(
    cx: &mut TestAppContext,
) {
    let def = workflow(
        vec![
            event_trigger(),
            session(
                "dev",
                json!({}),
                json!({"prompt": "Review {{trigger.item.ref}} {{trigger.item.title}} in {{trigger.item.repo}}"}),
            ),
        ],
        vec![edge("start", "dev")],
    );
    let rig = Rig::new(&["ok"]);
    assert!(rig.start_run(cx, event_run(&def)));
    cx.run_until_parked();
    let run = rig.last_run();
    assert!(
        rig.fake.world().starts[0]
            .prompt
            .starts_with("Review #7 Fix the thing in acme/app")
    );
    let fired = node(&run, "start");
    assert_eq!(fired.status, NodeStatus::Done);
    assert!(fired.output.as_ref().unwrap().get("title").is_some());
}
