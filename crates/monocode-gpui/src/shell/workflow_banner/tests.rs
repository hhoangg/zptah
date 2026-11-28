//! The banner of a "Wait for me" step and the terminal steps, over the real host and a test
//! shell. No provider runs: a chat step only gets its prompt in the composer, and a terminal one
//! is a pane whose program the test starts and ends itself.
use super::*;
use crate::monos::tests::{Setup, setup};
use crate::shell::app_commands::WORKFLOW_OUTPUT;
use crate::workflows::{WaitChoice, WaitNotice, Workflows};
use gpui::{Entity, TestAppContext, WindowHandle};
use monocode_core::{
    mono::workflows::{
        engine::start_run,
        model::{CallerKind, NodeStatus, RunCaller, RunStatus, Workflow, WorkflowRun},
        ordered::Json,
        parse::parse_workflow,
        store::load_runs,
    },
    surface::Surface,
};
use serde_json::{Value, json};
use std::sync::mpsc;

const WF: &str = "wf-abcd1234";

fn definition(s: &Setup, ship: Value) -> Workflow {
    let mut node = json!({
        "id": "ship", "type": "session", "project": "{{inputs.repo}}", "prompt": "/ship",
        "checkout": "root", "surface": "terminal",
    });
    for (key, value) in ship.as_object().unwrap() {
        node[key] = value.clone();
    }
    let def = json!({
        "id": WF, "name": "Ship it", "inputs": {"repo": {"type": "project"}},
        "inputValues": {"repo": s.project},
        "nodes": [{"id": "start", "type": "manual"}, node],
        "edges": [{"from": "start", "to": "ship"}],
        "enabled": true, "createdAt": 0,
    });
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

/// A test shell whose default harness is Claude, the only one a terminal step can run.
fn claude_setup(cx: &mut TestAppContext) -> Setup {
    let s = setup(cx);
    s.window
        .update(cx, |shell, _, _| {
            shell.workspace.preferences.default_provider = Some("claude".into());
        })
        .unwrap();
    s
}

fn workflows_of(s: &Setup, cx: &mut TestAppContext) -> Entity<Workflows> {
    s.window
        .update(cx, |shell, _, _| shell.workflows().expect("workflows"))
        .unwrap()
}

fn start(s: &Setup, cx: &mut TestAppContext, def: &Workflow) -> Entity<Workflows> {
    let workflows = workflows_of(s, cx);
    let run: WorkflowRun = start_run(
        def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    assert!(cx.update(|cx| workflows.update(cx, |w, cx| w.start_run(run, cx))));
    cx.run_until_parked();
    workflows
}

fn run_of(workflows: &Entity<Workflows>, cx: &mut TestAppContext) -> WorkflowRun {
    let dir = cx.update(|cx| workflows.read(cx).dir().clone());
    load_runs(&dir, WF).unwrap().remove(0)
}

fn ship_session(workflows: &Entity<Workflows>, cx: &mut TestAppContext) -> i64 {
    run_of(workflows, cx)
        .nodes
        .iter()
        .find(|n| n.node_id == "ship")
        .and_then(|n| n.session_id.clone())
        .expect("the step started a session")
        .parse()
        .expect("an unshared store keeps the plain id")
}

fn ship_status(workflows: &Entity<Workflows>, cx: &mut TestAppContext) -> NodeStatus {
    run_of(workflows, cx)
        .nodes
        .iter()
        .find(|n| n.node_id == "ship")
        .unwrap()
        .status
}

/// The CLI of the terminal session starts, then a turn goes (`busy`) or ends.
fn pane(
    s: &Setup,
    cx: &mut TestAppContext,
    id: i64,
    edit: impl FnOnce(
        &mut crate::tui_session::TuiSession,
        &mut gpui::Context<crate::tui_session::TuiSession>,
    ),
) {
    s.window
        .update(cx, |shell, _, cx| {
            let tui = shell.tui_sessions[&id].clone();
            tui.update(cx, edit);
        })
        .unwrap();
    cx.run_until_parked();
}

fn cli_started(s: &Setup, cx: &mut TestAppContext, id: i64) {
    s.window
        .update(cx, |shell, _, cx| {
            let terminal = shell.tui_sessions[&id]
                .read(cx)
                .terminal()
                .cloned()
                .expect("the CLI is launched");
            terminal.update(cx, |terminal, cx| terminal.fake_started(cx));
        })
        .unwrap();
    cx.run_until_parked();
}

fn turn(s: &Setup, cx: &mut TestAppContext, id: i64, busy: bool) {
    pane(s, cx, id, move |tui, cx| tui.set_busy(busy, cx));
}

fn call_output(
    window: WindowHandle<Shell>,
    session: i64,
    input: Value,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    let (reply, result) = mpsc::channel();
    window
        .update(cx, |shell, window, cx| {
            shell.handle_app_call(
                monocode_core::mono::bridge::Call {
                    session,
                    request_id: "out-1".into(),
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

// --- terminal steps ---------------------------------------------------------------------------

#[gpui::test]
fn a_terminal_step_opens_the_cli_with_its_prompt_in_a_background_tab(cx: &mut TestAppContext) {
    let s = claude_setup(cx);
    let active_before = s
        .window
        .update(cx, |shell, _, _| shell.workspace.active)
        .unwrap();
    let workflows = start(&s, cx, &definition(&s, json!({})));
    let id = ship_session(&workflows, cx);
    s.window
        .update(cx, |shell, _, cx| {
            assert_eq!(shell.session_summaries[&id].surface, Surface::Tui);
            assert_eq!(shell.session_summaries[&id].provider, "claude");
            assert!(!shell.sessions.contains_key(&id), "a terminal has no chat");
            let terminal = shell.tui_sessions[&id].read(cx).terminal().unwrap();
            let args = terminal.read(cx).launched().unwrap().args.clone();
            assert_eq!(args.last().unwrap().to_string_lossy(), "/ship");
            // A tab without the focus: the user's view did not move.
            assert!(shell.workspace.tabs.contains(&id));
            assert_eq!(shell.workspace.active, active_before);
            // Nothing shows it, and the pane is kept all the same.
            shell.sync_tui(&std::collections::HashSet::new(), cx);
            assert!(
                shell.tui_sessions.contains_key(&id),
                "sync_tui kept the CLI"
            );
            assert!(shell.session_zap(id), "it wears the workflow badge");
        })
        .unwrap();
    assert_eq!(ship_status(&workflows, cx), NodeStatus::Running);
}

#[gpui::test]
fn a_terminal_step_takes_the_hint_after_its_slash_command(cx: &mut TestAppContext) {
    let s = claude_setup(cx);
    let workflows = start(
        &s,
        cx,
        &definition(&s, json!({"outputs": {"pr": {"type": "string"}}})),
    );
    let id = ship_session(&workflows, cx);
    s.window
        .update(cx, |shell, _, cx| {
            let terminal = shell.tui_sessions[&id].read(cx).terminal().unwrap();
            let args = terminal.read(cx).launched().unwrap().args.clone();
            let prompt = args.last().unwrap().to_string_lossy().into_owned();
            assert!(prompt.starts_with("/ship"), "{prompt}");
            assert!(prompt.contains("app workflow.output"), "{prompt}");
        })
        .unwrap();
}

#[gpui::test]
fn a_busy_turn_that_ends_finishes_a_terminal_step(cx: &mut TestAppContext) {
    let s = claude_setup(cx);
    let workflows = start(&s, cx, &definition(&s, json!({})));
    let id = ship_session(&workflows, cx);
    cli_started(&s, cx, id);
    // Starting up is not a turn: the step goes on waiting for one.
    assert_eq!(ship_status(&workflows, cx), NodeStatus::Running);
    turn(&s, cx, id, true);
    assert_eq!(ship_status(&workflows, cx), NodeStatus::Running);
    turn(&s, cx, id, false);
    let run = run_of(&workflows, cx);
    assert_eq!(run.status, RunStatus::Done);
    let ship = run.nodes.iter().find(|n| n.node_id == "ship").unwrap();
    assert_eq!(ship.status, NodeStatus::Done);
    let output = ship.output.as_ref().unwrap();
    assert!(output.contains_key("sessionId") && !output.contains_key("reply"));
    // The CLI is still there for the user.
    s.window
        .update(cx, |shell, _, _| {
            assert!(shell.tui_sessions.contains_key(&id))
        })
        .unwrap();
}

#[gpui::test]
fn workflow_output_from_the_terminal_session_is_recorded(cx: &mut TestAppContext) {
    let s = claude_setup(cx);
    let workflows = start(
        &s,
        cx,
        &definition(&s, json!({"outputs": {"pr": {"type": "string"}}})),
    );
    let id = ship_session(&workflows, cx);
    cli_started(&s, cx, id);
    turn(&s, cx, id, true);
    let reply = call_output(
        s.window,
        id,
        json!({"pr": "https://example.invalid/pr/1"}),
        cx,
    )
    .unwrap();
    assert_eq!(reply["step"], "ship");
    turn(&s, cx, id, false);
    let run = run_of(&workflows, cx);
    assert_eq!(run.status, RunStatus::Done);
    let output = run
        .nodes
        .iter()
        .find(|n| n.node_id == "ship")
        .and_then(|n| n.output.clone())
        .unwrap();
    assert_eq!(
        output.get("pr").and_then(|v| v.as_str()),
        Some("https://example.invalid/pr/1")
    );
}

#[gpui::test]
fn a_terminal_step_that_recorded_nothing_fails_with_a_clear_reason(cx: &mut TestAppContext) {
    let s = claude_setup(cx);
    let workflows = start(
        &s,
        cx,
        &definition(&s, json!({"outputs": {"pr": {"type": "string"}}})),
    );
    let id = ship_session(&workflows, cx);
    cli_started(&s, cx, id);
    turn(&s, cx, id, true);
    turn(&s, cx, id, false);
    let run = run_of(&workflows, cx);
    assert_eq!(run.status, RunStatus::Failed);
    let error = run
        .nodes
        .iter()
        .find(|n| n.node_id == "ship")
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert!(
        error.contains("workflow.output") && error.contains("pr"),
        "{error}"
    );
}

#[gpui::test]
fn a_cli_that_exits_before_any_turn_fails_the_step(cx: &mut TestAppContext) {
    let s = claude_setup(cx);
    let workflows = start(&s, cx, &definition(&s, json!({})));
    let id = ship_session(&workflows, cx);
    s.window
        .update(cx, |shell, _, cx| {
            let terminal = shell.tui_sessions[&id]
                .read(cx)
                .terminal()
                .cloned()
                .unwrap();
            terminal.update(cx, |terminal, cx| terminal.fake_exit(Some(1), cx));
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(run_of(&workflows, cx).status, RunStatus::Failed);
}

#[gpui::test]
fn closing_the_pane_fails_the_step(cx: &mut TestAppContext) {
    let s = claude_setup(cx);
    let workflows = start(&s, cx, &definition(&s, json!({})));
    let id = ship_session(&workflows, cx);
    s.window
        .update(cx, |shell, _, cx| shell.stop_tui(id, cx))
        .unwrap();
    cx.run_until_parked();
    let run = run_of(&workflows, cx);
    assert_eq!(run.status, RunStatus::Failed);
    let error = run
        .nodes
        .iter()
        .find(|n| n.node_id == "ship")
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert!(error.contains("closed"), "{error}");
}

#[gpui::test]
fn a_terminal_step_that_would_not_run_claude_is_refused(cx: &mut TestAppContext) {
    let s = setup(cx);
    // The app's default harness is not Claude and the step names none.
    s.window
        .update(cx, |shell, _, _| {
            shell.workspace.preferences.default_provider = Some("codex".into());
        })
        .unwrap();
    let workflows = start(&s, cx, &definition(&s, json!({})));
    let run = run_of(&workflows, cx);
    assert_eq!(run.status, RunStatus::Failed);
    let error = run
        .nodes
        .iter()
        .find(|n| n.node_id == "ship")
        .unwrap()
        .error
        .clone()
        .unwrap();
    assert!(error.contains("Claude only"), "{error}");
    // No session was made for it.
    s.window
        .update(cx, |shell, _, _| assert!(shell.tui_sessions.is_empty()))
        .unwrap();
}

// --- the banner -------------------------------------------------------------------------------

/// A session in the project that no step runs, as a chat or as a terminal.
fn session_of(s: &Setup, surface: Surface, cx: &mut TestAppContext) -> i64 {
    s.window
        .update(cx, |shell, _, cx| {
            let summary = crate::runtime::locked(&shell.store)
                .unwrap()
                .create_with_surface("claude", &s.project, surface)
                .unwrap();
            let id = summary.id;
            shell.add(summary, cx);
            id
        })
        .unwrap()
}

fn notice(id: i64) -> WaitNotice {
    WaitNotice {
        session_id: id.to_string(),
        workflow: "Ship it".into(),
        step: "review".into(),
    }
}

#[gpui::test]
fn a_waiting_step_shows_a_banner_in_a_chat_and_a_terminal_session_and_tells_the_user(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let workflows = workflows_of(&s, cx);
    let host = cx.update(|cx| workflows.update(cx, |w, cx| w.workflow_host(cx)));
    for surface in [Surface::Chat, Surface::Tui] {
        let id = session_of(&s, surface, cx);
        host.wait_for_user(notice(id));
        cx.run_until_parked();
        let banner = cx.update(|cx| workflows.read(cx).wait_banner(id));
        assert_eq!(
            banner.map(|b| (b.workflow, b.step)),
            Some(("Ship it".into(), "review".into()))
        );
        let notices = cx.update(|cx| workflows.read(cx).needs_you());
        assert!(
            notices.contains(&(id, "Workflow Ship it - step review needs you".into())),
            "{notices:?}"
        );
        // It is drawn at the top of the session, with both buttons.
        s.window
            .update(cx, |shell, window, cx| {
                shell.activate_session(id, window, cx)
            })
            .unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(s.window.into(), cx);
        visual.run_until_parked();
        for part in [
            "workflow-wait-banner",
            "workflow-wait-continue",
            "workflow-wait-fail",
        ] {
            assert!(visual.debug_bounds(part).is_some(), "{surface:?}: {part}");
        }
        // The step is over (Stop, an answer, a timeout): the banner goes.
        host.wait_over(&id.to_string());
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("workflow-wait-banner").is_none(),
            "{surface:?}"
        );
    }
}

#[gpui::test]
fn a_banner_button_for_a_step_that_is_gone_only_removes_the_banner(cx: &mut TestAppContext) {
    let s = setup(cx);
    let workflows = workflows_of(&s, cx);
    let host = cx.update(|cx| workflows.update(cx, |w, cx| w.workflow_host(cx)));
    let id = session_of(&s, Surface::Chat, cx);
    host.wait_for_user(notice(id));
    cx.run_until_parked();
    for choice in [WaitChoice::Continue, WaitChoice::Fail] {
        s.window
            .update(cx, |shell, _, cx| {
                shell.workflow_answer_wait(id, choice, cx)
            })
            .unwrap();
    }
    assert!(cx.update(|cx| workflows.read(cx).wait_banner(id)).is_none());
}
