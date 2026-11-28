//! `workflow.output` through the app bridge handler: the value lands in the caller's own node,
//! and a session that is not an active step is refused. No provider runs: the step's session
//! only gets its prompt in the composer (`Probes.draft_sessions`), so the run stays going.
use super::*;
use crate::monos::tests::{Setup, setup};
use crate::shell::app_commands::WORKFLOW_OUTPUT;
use gpui::{TestAppContext, WindowHandle};
use monocode_core::{
    mono::workflows::{
        engine::start_run,
        model::{CallerKind, RunCaller, Workflow, WorkflowRun},
        ordered::Json,
        parse::parse_workflow,
        store::load_runs,
    },
    surface::Surface,
};
use serde_json::{Value, json};
use std::sync::mpsc;

const WF: &str = "wf-abcd1234";

fn definition(s: &Setup) -> Workflow {
    let def = json!({
        "id": WF, "name": "Outputs", "inputs": {"repo": {"type": "project"}},
        "inputValues": {"repo": s.project},
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "dev", "type": "session", "project": "{{inputs.repo}}", "prompt": "Decide",
             "checkout": "root",
             "outputs": {"approved": {"type": "boolean"}, "note": {"type": "string"}}},
        ],
        "edges": [{"from": "start", "to": "dev"}],
        "enabled": true, "createdAt": 0,
    });
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

fn call(
    window: WindowHandle<Shell>,
    session: i64,
    request: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    let (reply, result) = mpsc::channel();
    window
        .update(cx, |shell, window, cx| {
            shell.handle_app_call(
                Call {
                    session,
                    request_id: request.into(),
                    action: WORKFLOW_OUTPUT.into(),
                    input,
                    reply,
                },
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    result.try_recv().expect("the action must settle")
}

fn running(s: &Setup, cx: &mut TestAppContext) -> (i64, gpui::Entity<crate::workflows::Workflows>) {
    let workflows = s
        .window
        .update(cx, |shell, _, _| shell.workflows().expect("workflows"))
        .unwrap();
    let run: WorkflowRun = start_run(
        &definition(s),
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    assert!(cx.update(|cx| workflows.update(cx, |w, cx| w.start_run(run, cx))));
    cx.run_until_parked();
    let dir = cx.update(|cx| workflows.read(cx).dir().clone());
    let runs = load_runs(&dir, WF).unwrap();
    let session = runs[0]
        .nodes
        .iter()
        .find(|n| n.node_id == "dev")
        .and_then(|n| n.session_id.clone())
        .expect("the step started a session");
    (
        session
            .parse()
            .expect("an unshared store keeps the plain id"),
        workflows,
    )
}

fn saved_pending(
    workflows: &gpui::Entity<crate::workflows::Workflows>,
    cx: &mut TestAppContext,
) -> Option<Value> {
    let dir = cx.update(|cx| workflows.read(cx).dir().clone());
    load_runs(&dir, WF).unwrap()[0]
        .nodes
        .iter()
        .find(|n| n.node_id == "dev")
        .and_then(|n| n.pending_output.as_ref())
        .map(|p| Json::Object(p.clone()).to_value())
}

/// A second session in the same project that no step runs, as a chat or as a terminal.
fn stranger(s: &Setup, surface: Surface, cx: &mut TestAppContext) -> i64 {
    s.window
        .update(cx, |shell, _, cx| {
            let summary = locked(&shell.store)
                .unwrap()
                .create_with_surface("claude", &s.project, surface)
                .unwrap();
            let id = summary.id;
            shell.add(summary, cx);
            id
        })
        .unwrap()
}

#[gpui::test]
fn the_value_lands_in_the_node_of_the_calling_session(cx: &mut TestAppContext) {
    let s = setup(cx);
    let (session, workflows) = running(&s, cx);
    let reply = call(s.window, session, "r1", json!({"approved": true}), cx).unwrap();
    assert_eq!(reply["step"], "dev");
    assert_eq!(reply["recorded"], json!(["approved"]));
    assert_eq!(reply["missing"], json!(["note"]));
    // A second call keeps the first field and adds the other; the last value of a field wins.
    call(
        s.window,
        session,
        "r2",
        json!({"note": "ok", "approved": false}),
        cx,
    )
    .unwrap();
    assert_eq!(
        saved_pending(&workflows, cx),
        Some(json!({"approved": false, "note": "ok"}))
    );
    // Sending the same request again returns the saved answer instead of writing twice.
    let again = call(s.window, session, "r1", json!({"approved": true}), cx).unwrap();
    assert_eq!(again["recorded"], json!(["approved"]));
    assert_eq!(
        saved_pending(&workflows, cx),
        Some(json!({"approved": false, "note": "ok"}))
    );
}

#[gpui::test]
fn bad_values_are_refused_and_change_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    let (session, workflows) = running(&s, cx);
    let wrong = call(s.window, session, "w1", json!({"approved": "yes"}), cx).unwrap_err();
    assert!(
        wrong.contains("approved") && wrong.contains("true or false"),
        "{wrong}"
    );
    let extra = call(s.window, session, "w2", json!({"other": 1}), cx).unwrap_err();
    assert!(extra.contains("approved, note"), "{extra}");
    assert_eq!(saved_pending(&workflows, cx), None);
}

#[gpui::test]
fn a_chat_or_terminal_session_that_is_not_a_step_is_refused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let (_, workflows) = running(&s, cx);
    for (request, surface) in [("c", Surface::Chat), ("t", Surface::Tui)] {
        let other = stranger(&s, surface, cx);
        let error = call(s.window, other, request, json!({"approved": true}), cx).unwrap_err();
        assert!(error.contains("not running a workflow step"), "{error}");
    }
    assert_eq!(saved_pending(&workflows, cx), None);
    // A session the app does not have at all.
    let error = call(s.window, 987_654, "x", json!({"approved": true}), cx).unwrap_err();
    assert_eq!(error, "Unknown session.");
}

#[gpui::test]
fn a_call_after_the_run_stopped_is_refused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let (session, workflows) = running(&s, cx);
    assert!(cx.update(|cx| workflows.update(cx, |w, _| w.stop_run(WF))));
    cx.run_until_parked();
    let error = call(s.window, session, "late", json!({"approved": true}), cx).unwrap_err();
    assert!(error.contains("workflow step"), "{error}");
}
