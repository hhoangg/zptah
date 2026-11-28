//! The caller of `workflow.output` is placed by its session id alone: concurrent runs, For each
//! branches and loop rounds must not be able to write into each other.
use super::active::*;
use super::engine::*;
use super::fixtures::{edge, node};
use super::model::*;
use super::ordered::Json;
use serde_json::{json, Value};

const AT: u64 = 1_000;

fn n(v: Value) -> Node {
    node(&v.to_string())
}

fn session(id: &str, extra: Value) -> Node {
    let mut v = json!({
        "id": id, "type": "session", "project": "{{inputs.project}}", "checkout": "root",
        "prompt": format!("Do {id}"), "outputs": {"ok": {"type": "boolean"}, "note": {"type": "string"}},
    });
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    n(v)
}

fn workflow(id: &str, nodes: Vec<Node>, edges: Vec<Edge>) -> Workflow {
    let mut w = Workflow::new(id, "Test");
    w.inputs
        .insert("project", WorkflowInput::new(InputType::Project));
    w.input_values
        .insert("project", InputValue::String("/work/repo".into()));
    w.nodes = nodes;
    w.edges = edges;
    w.enabled = true;
    w
}

fn e(from: &str, to: &str) -> Edge {
    edge(&json!({"from": from, "to": to}).to_string())
}

fn single(workflow_id: &str, run_id: &str) -> WorkflowRun {
    let w = workflow(
        workflow_id,
        vec![
            n(json!({"id": "start", "type": "manual"})),
            session("dev", json!({})),
        ],
        vec![e("start", "dev")],
    );
    start_run(&w, RunCaller::new(CallerKind::Manual, "start"), AT, run_id)
}

fn started(run: &WorkflowRun, id: &str, branch: &str, session_id: &str) -> WorkflowRun {
    apply_event(
        run,
        &EngineEvent::NodeStarted {
            node_id: id.into(),
            branch: branch.into(),
            session_id: Some(session_id.into()),
            at: AT,
        },
    )
}

fn finished(run: &WorkflowRun, id: &str, branch: &str, output: Value) -> WorkflowRun {
    let Json::Object(output) = Json::from(&output) else {
        panic!("not an object")
    };
    apply_event(
        run,
        &EngineEvent::NodeFinished {
            node_id: id.into(),
            branch: branch.into(),
            output,
            at: AT,
        },
    )
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn value(v: Value) -> Json {
    Json::from(&v)
}

fn pending(run: &WorkflowRun, node: &str, branch: &str) -> Option<Value> {
    run.nodes
        .iter()
        .find(|r| r.node_id == node && r.branch == branch)
        .and_then(|r| r.pending_output.as_ref())
        .map(|p| Json::Object(p.clone()).to_value())
}

#[test]
fn a_session_is_found_by_its_local_id_or_by_its_persisted_id() {
    let run = started(&single("wf-1", "r1"), "dev", "", "remote:abc");
    let want = ActiveNode {
        workflow_id: "wf-1".into(),
        run_id: "r1".into(),
        node_id: "dev".into(),
        branch: String::new(),
        round: 1,
    };
    // The caller sends its local id and the app also looks the persisted one up.
    assert_eq!(
        find_active(&[&run], &ids(&["12", "remote:abc"])),
        Ok(want.clone())
    );
    assert_eq!(find_active(&[&run], &ids(&["remote:abc"])), Ok(want));
    // The local id alone is not what the file stores.
    assert_eq!(
        find_active(&[&run], &ids(&["12"])),
        Err(ActiveError::NotAStep)
    );
}

#[test]
fn a_stranger_session_is_refused_with_a_clear_message() {
    let run = started(&single("wf-1", "r1"), "dev", "", "7");
    let err = find_active(&[&run], &ids(&["99"])).unwrap_err();
    assert_eq!(err, ActiveError::NotAStep);
    assert!(err.to_string().contains("not running a workflow step"));
    assert_eq!(find_active(&[], &ids(&["7"])), Err(ActiveError::NotAStep));
}

#[test]
fn a_finished_node_a_pending_node_and_a_stopped_run_are_not_active() {
    let run = started(&single("wf-1", "r1"), "dev", "", "7");
    let done = finished(&run, "dev", "", json!({"ok": true, "note": "x"}));
    assert_eq!(find_active(&[&done], &ids(&["7"])), Err(ActiveError::Ended));
    // Not started yet: no session either way.
    assert_eq!(
        find_active(&[&single("wf-1", "r1")], &ids(&["7"])),
        Err(ActiveError::NotAStep)
    );
    let stopped = apply_event(&run, &EngineEvent::Stopped { at: AT });
    assert_eq!(
        find_active(&[&stopped], &ids(&["7"])),
        Err(ActiveError::Ended)
    );
    let mut over = run.clone();
    over.status = RunStatus::Done;
    assert_eq!(find_active(&[&over], &ids(&["7"])), Err(ActiveError::Ended));
}

#[test]
fn a_waiting_for_me_node_is_still_active() {
    let run = started(&single("wf-1", "r1"), "dev", "", "7");
    let waiting = apply_event(
        &run,
        &EngineEvent::WaitingForUser {
            node_id: "dev".into(),
            branch: String::new(),
            waiting: true,
        },
    );
    assert_eq!(waiting.nodes[1].status, NodeStatus::WaitingForUser);
    assert!(find_active(&[&waiting], &ids(&["7"])).is_ok());
    let approval = apply_event(
        &run,
        &EngineEvent::WaitingApproval {
            node_id: "dev".into(),
            branch: String::new(),
            waiting: true,
        },
    );
    assert!(find_active(&[&approval], &ids(&["7"])).is_ok());
}

#[test]
fn for_each_branches_are_isolated_by_their_sessions() {
    let w = workflow(
        "wf-1",
        vec![
            n(json!({"id": "start", "type": "manual"})),
            session("fetch", json!({"outputs": {"tickets": {"type": "list"}}})),
            n(json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"})),
            session("dev", json!({"parent": "each"})),
        ],
        vec![e("start", "fetch"), e("fetch", "each")],
    );
    let mut run = start_run(&w, RunCaller::new(CallerKind::Manual, "start"), AT, "r1");
    run = started(&run, "fetch", "", "1");
    run = finished(&run, "fetch", "", json!({"tickets": [1, 2]}));
    run = started(&run, "dev", "each#0", "10");
    run = started(&run, "dev", "each#1", "11");
    let a = find_active(&[&run], &ids(&["10"])).unwrap();
    let b = find_active(&[&run], &ids(&["11"])).unwrap();
    assert_eq!((a.node_id.as_str(), a.branch.as_str()), ("dev", "each#0"));
    assert_eq!((b.node_id.as_str(), b.branch.as_str()), ("dev", "each#1"));
    // Branch 0 finishes: its session is over while branch 1's goes on.
    run = finished(&run, "dev", "each#0", json!({"ok": true, "note": "a"}));
    assert_eq!(find_active(&[&run], &ids(&["10"])), Err(ActiveError::Ended));
    assert!(find_active(&[&run], &ids(&["11"])).is_ok());
    // A write goes to the branch of the caller only.
    let after = record_output(&run, &b, &value(json!({"ok": false}))).unwrap();
    assert_eq!(pending(&after, "dev", "each#1"), Some(json!({"ok": false})));
    assert_eq!(pending(&after, "dev", "each#0"), None);
    assert_eq!(pending(&after, "fetch", ""), None);
}

#[test]
fn two_concurrent_runs_are_isolated() {
    let one = started(&single("wf-1", "r1"), "dev", "", "7");
    let two = started(&single("wf-1", "r2"), "dev", "", "8");
    let a = find_active(&[&one, &two], &ids(&["7"])).unwrap();
    let b = find_active(&[&one, &two], &ids(&["8"])).unwrap();
    assert_eq!((a.run_id.as_str(), b.run_id.as_str()), ("r1", "r2"));
    let written = record_output(&two, &b, &value(json!({"note": "second"}))).unwrap();
    assert_eq!(
        pending(&written, "dev", ""),
        Some(json!({"note": "second"}))
    );
    assert_eq!(pending(&one, "dev", ""), None);
    // The target of one run cannot be written into another run's record.
    assert!(record_output(&one, &b, &value(json!({"note": "x"}))).is_err());
}

#[test]
fn another_workflow_with_the_same_session_id_is_ambiguous_not_guessed() {
    let one = started(&single("wf-1", "r1"), "dev", "", "7");
    let two = started(&single("wf-2", "r2"), "dev", "", "7");
    assert_eq!(
        find_active(&[&one, &two], &ids(&["7"])),
        Err(ActiveError::Ambiguous)
    );
}

#[test]
fn a_later_loop_round_with_a_new_session_ends_the_old_one() {
    let w = workflow(
        "wf-1",
        vec![
            n(json!({"id": "start", "type": "manual"})),
            session("dev", json!({"reuseSession": false})),
            session(
                "review",
                json!({"outputs": {"verdict": {"type": "string"}}}),
            ),
        ],
        vec![
            e("start", "dev"),
            e("dev", "review"),
            edge(
                &json!({"from": "review", "to": "dev",
                    "loop": {"maxRounds": 3, "decide": "{{review.verdict}} != \"approve\""}})
                .to_string(),
            ),
        ],
    );
    let mut run = start_run(&w, RunCaller::new(CallerKind::Manual, "start"), AT, "r1");
    run = started(&run, "dev", "", "7");
    let first = find_active(&[&run], &ids(&["7"])).unwrap();
    run = record_output(&run, &first, &value(json!({"ok": true}))).unwrap();
    run = finished(&run, "dev", "", json!({"ok": true}));
    run = started(&run, "review", "", "8");
    run = finished(&run, "review", "", json!({"verdict": "changes"}));
    // Round 2 of dev: the engine reset the node. Outputs of round 1 are gone.
    assert_eq!(
        run.nodes.iter().find(|r| r.node_id == "dev").unwrap().round,
        2
    );
    assert_eq!(pending(&run, "dev", ""), None);
    // Between rounds the old session is not active, and a call from it is refused.
    assert_eq!(find_active(&[&run], &ids(&["7"])), Err(ActiveError::Ended));
    assert!(record_output(&run, &first, &value(json!({"ok": false}))).is_err());
    // The new session of round 2 is the active one, and round 1's target no longer fits.
    run = started(&run, "dev", "", "9");
    let second = find_active(&[&run], &ids(&["9"])).unwrap();
    assert_eq!(second.round, 2);
    assert_eq!(find_active(&[&run], &ids(&["7"])), Err(ActiveError::Ended));
    assert!(record_output(&run, &first, &value(json!({"ok": false}))).is_err());
    assert!(record_output(&run, &second, &value(json!({"ok": false}))).is_ok());
}

#[test]
fn a_reused_session_starts_each_round_with_empty_outputs() {
    let w = workflow(
        "wf-1",
        vec![
            n(json!({"id": "start", "type": "manual"})),
            session("dev", json!({})),
            session(
                "review",
                json!({"outputs": {"verdict": {"type": "string"}}}),
            ),
        ],
        vec![
            e("start", "dev"),
            e("dev", "review"),
            edge(
                &json!({"from": "review", "to": "dev",
                    "loop": {"maxRounds": 3, "decide": "{{review.verdict}} != \"approve\""}})
                .to_string(),
            ),
        ],
    );
    let mut run = start_run(&w, RunCaller::new(CallerKind::Manual, "start"), AT, "r1");
    run = started(&run, "dev", "", "7");
    let target = find_active(&[&run], &ids(&["7"])).unwrap();
    run = record_output(&run, &target, &value(json!({"ok": true, "note": "one"}))).unwrap();
    // A second call keeps the other field and replaces the one it names.
    run = record_output(&run, &target, &value(json!({"note": "two"}))).unwrap();
    assert_eq!(
        pending(&run, "dev", ""),
        Some(json!({"ok": true, "note": "two"}))
    );
    run = finished(&run, "dev", "", json!({"ok": true, "note": "two"}));
    run = started(&run, "review", "", "8");
    run = finished(&run, "review", "", json!({"verdict": "changes"}));
    // The engine sends the same session back for round 2: nothing left over from round 1.
    let reuse = next_actions(&run).into_iter().find_map(|a| match a {
        EngineAction::StartSession {
            reuse_session_id, ..
        } => reuse_session_id,
        _ => None,
    });
    assert_eq!(reuse.as_deref(), Some("7"));
    run = started(&run, "dev", "", "7");
    assert_eq!(pending(&run, "dev", ""), None);
    let again = find_active(&[&run], &ids(&["7"])).unwrap();
    assert_eq!(again.round, 2);
}

#[test]
fn a_call_after_the_node_finished_is_refused() {
    let run = started(&single("wf-1", "r1"), "dev", "", "7");
    let target = find_active(&[&run], &ids(&["7"])).unwrap();
    let done = finished(&run, "dev", "", json!({"ok": true, "note": "x"}));
    let err = record_output(&done, &target, &value(json!({"ok": false}))).unwrap_err();
    assert!(err.contains("already ended"), "{err}");
}

#[test]
fn values_are_type_checked_and_undeclared_fields_are_refused_with_the_allowed_list() {
    let run = started(&single("wf-1", "r1"), "dev", "", "7");
    let target = find_active(&[&run], &ids(&["7"])).unwrap();
    let wrong = record_output(&run, &target, &value(json!({"ok": "yes"}))).unwrap_err();
    assert!(
        wrong.contains("\"ok\"") && wrong.contains("true or false"),
        "{wrong}"
    );
    let extra = record_output(&run, &target, &value(json!({"other": 1}))).unwrap_err();
    assert!(
        extra.contains("\"other\"") && extra.contains("note, ok"),
        "{extra}"
    );
    // Nothing was written by the refused calls.
    assert_eq!(pending(&run, "dev", ""), None);
}

#[test]
fn a_node_that_is_only_being_asked_about_its_loop_is_not_active() {
    let w = workflow(
        "wf-1",
        vec![
            n(json!({"id": "start", "type": "manual"})),
            session("dev", json!({})),
            session(
                "review",
                json!({"outputs": {"verdict": {"type": "string"}}}),
            ),
        ],
        vec![
            e("start", "dev"),
            e("dev", "review"),
            edge(
                &json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "ai"}})
                    .to_string(),
            ),
        ],
    );
    let mut run = start_run(&w, RunCaller::new(CallerKind::Manual, "start"), AT, "r1");
    run = started(&run, "dev", "", "7");
    run = finished(&run, "dev", "", json!({"ok": true, "note": "x"}));
    run = started(&run, "review", "", "8");
    run = finished(&run, "review", "", json!({"verdict": "changes"}));
    // The loop question goes out: the node is `running` again with an end time.
    run = started(&run, "review", "", "8");
    assert_eq!(find_active(&[&run], &ids(&["8"])), Err(ActiveError::Ended));
}
