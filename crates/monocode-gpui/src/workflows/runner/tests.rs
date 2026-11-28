//! Ports of the run-executing cases of `useMonoWorkflows.test.ts` (against a fake host),
//! `workflowSessionStart.test.ts` and the real host over a test `Monos`.
use super::text::*;
use super::*;
use async_channel::{Receiver, Sender, unbounded};
use gpui::TestAppContext;
use monocode_core::mono::workflows::{
    command::CommandResult,
    engine::start_run,
    model::{CallerKind, DecisionModel, RunCaller, Workflow},
    ordered::Json,
    parse::parse_workflow,
    validate::validate_workflow,
};
use serde_json::{Value, json};
use std::collections::VecDeque;

mod cloud;
mod data;
mod event;
mod fake;
mod real;
mod workspace;
use fake::*;

// --- pure helpers ---------------------------------------------------------------------------

#[test]
fn cap_output_cuts_long_strings_and_says_where() {
    let long = "x".repeat(20_000);
    let capped = cap_string(&long);
    assert!(capped.starts_with(&"x".repeat(16_000)));
    assert!(capped.ends_with("\n[truncated, 4000 more characters]"));
    assert_eq!(cap_string("short"), "short");
    assert_eq!(cap_string(&"y".repeat(16_000)).len(), 16_000);
}

#[test]
fn cap_output_walks_lists_and_objects_but_leaves_other_values() {
    let long = "z".repeat(16_001);
    let value =
        Json::parse(&json!({"a": [long, {"b": long}], "n": 3, "t": true, "u": null}).to_string())
            .unwrap();
    let capped = cap_output(&value).stringify(false);
    assert_eq!(capped.matches("[truncated, 1 more characters]").count(), 2);
    assert!(
        capped.contains("\"n\":3")
            && capped.contains("\"t\":true")
            && capped.contains("\"u\":null")
    );
    // Past 24 levels of nesting the value is kept as it is, like the TS walk.
    let mut deep = Json::String(long.clone());
    for _ in 0..30 {
        deep = Json::Array(vec![deep]);
    }
    assert!(cap_output(&deep).stringify(false).contains(&long));
}

#[test]
fn cap_output_counts_utf16_units_like_js() {
    // Each of these is two UTF-16 units: 8,001 of them are 16,002 units.
    let text = "😀".repeat(8_001);
    let capped = cap_string(&text);
    assert!(capped.starts_with(&"😀".repeat(8_000)));
    assert!(capped.ends_with("[truncated, 2 more characters]"));
    // A cut in the middle of a pair drops the pair instead of making a lone surrogate.
    let odd = format!("a{}", "😀".repeat(8_000));
    assert!(cap_string(&odd).starts_with(&format!("a{}", "😀".repeat(7_999))));
}

#[test]
fn errors_keep_their_end_behind_an_ellipsis() {
    assert_eq!(clip("short"), "short");
    let long = format!("{}END", "a".repeat(2_000));
    let clipped = clip(&long);
    assert!(clipped.starts_with('…') && clipped.ends_with("aaEND"));
    assert_eq!(clipped.encode_utf16().count(), 1_001);
}

#[test]
fn utf16_slices_never_split_a_character() {
    assert_eq!(utf16_head("a😀b", 2), "a");
    assert_eq!(utf16_head("a😀b", 3), "a😀");
    assert_eq!(utf16_tail("a😀b", 2), "b");
    assert_eq!(utf16_tail("a😀b", 3), "😀b");
    assert_eq!(utf16_head("abc", 10), "abc");
    assert_eq!(utf16_tail("abc", 10), "abc");
}

#[test]
fn the_workflow_folder_is_named_after_the_workflow() {
    assert_eq!(workflow_folder_name("Ship it"), "Ship it");
    // White space runs collapse and the ends are trimmed.
    assert_eq!(workflow_folder_name("  Ship \n\t it  "), "Ship it");
    assert_eq!(workflow_folder_name("   "), "Workflow");
}

#[test]
fn the_workflow_folder_name_stays_within_100_characters() {
    let name = workflow_folder_name(&"w".repeat(300));
    assert_eq!(name.encode_utf16().count(), 100);
    let wide = workflow_folder_name(&"😀".repeat(300));
    assert!(wide.encode_utf16().count() <= 100);
}

#[test]
fn a_turn_maps_to_the_outcome_a_run_reads() {
    use crate::conversation::{TurnEnd, WorkflowTurn};
    let turn = |end| WorkflowTurn {
        end,
        reply: "said".into(),
    };
    assert_eq!(
        host::outcome_of(turn(TurnEnd::Completed)),
        Outcome::completed("said")
    );
    let cancelled = host::outcome_of(turn(TurnEnd::Cancelled));
    assert_eq!(cancelled.status, Status::Cancelled);
    let failed = host::outcome_of(turn(TurnEnd::Failed("No network".into())));
    assert_eq!(failed.status, Status::Failed);
    assert_eq!(failed.error.as_deref(), Some("No network"));
}

#[test]
fn a_session_not_seen_working_is_given_up_on_unless_it_failed_to_start() {
    use crate::conversation::{TurnEnd, WorkflowTurn};
    use host::{Seen, look};
    let over = |end| {
        Seen::Over(WorkflowTurn {
            end,
            reply: String::new(),
        })
    };
    let (mut armed, mut unstarted) = (false, 0);
    // A prompt that failed to start says why at once, with the conversation's own status.
    let failed = look(
        over(TurnEnd::Failed("No network".into())),
        &mut armed,
        &mut unstarted,
    );
    assert_eq!(failed.unwrap().error.as_deref(), Some("No network"));
    // An idle session that was never working: patience, then a failure.
    for _ in 0..10 {
        assert_eq!(
            look(over(TurnEnd::Completed), &mut armed, &mut unstarted),
            None
        );
    }
    let late = look(over(TurnEnd::Completed), &mut armed, &mut unstarted).unwrap();
    assert_eq!(
        late.error.as_deref(),
        Some("The session did not start a turn.")
    );
    // Once seen working, idle means the turn is over.
    assert_eq!(look(Seen::Working, &mut armed, &mut unstarted), None);
    assert_eq!(
        look(over(TurnEnd::Completed), &mut armed, &mut unstarted)
            .unwrap()
            .status,
        Status::Completed
    );
}

#[test]
fn a_cloud_session_that_is_still_starting_is_waited_for_without_end() {
    use crate::conversation::{TurnEnd, WorkflowTurn};
    use host::{Seen, look};
    let idle = || {
        Seen::Over(WorkflowTurn {
            end: TurnEnd::Completed,
            reply: String::new(),
        })
    };
    let (mut armed, mut unstarted) = (false, 0);
    // Far more looks than the patience for a turn that never begins, all while the start runs.
    for _ in 0..500 {
        assert_eq!(look(Seen::Starting, &mut armed, &mut unstarted), None);
    }
    assert!(!armed, "a start is not a turn");
    // Once it is over the usual patience for the turn to appear begins from nothing.
    for _ in 0..10 {
        assert_eq!(look(idle(), &mut armed, &mut unstarted), None);
    }
    // A start that comes round again (a retry) gives the patience back.
    assert_eq!(look(Seen::Starting, &mut armed, &mut unstarted), None);
    for _ in 0..10 {
        assert_eq!(look(idle(), &mut armed, &mut unstarted), None);
    }
    assert!(look(idle(), &mut armed, &mut unstarted).is_some());
}

// --- the primitives -------------------------------------------------------------------------

#[gpui::test]
fn the_gate_hands_out_turns_in_order(cx: &mut TestAppContext) {
    let gate = Gate::default();
    let log = Rc::new(RefCell::new(Vec::new()));
    let (release, wait) = unbounded::<()>();
    for name in ["a", "b", "c"] {
        let (gate, log, wait) = (gate.clone(), log.clone(), wait.clone());
        cx.spawn(async move |_| {
            let _turn = gate.turn().await;
            log.borrow_mut().push(format!("{name} in"));
            let _ = wait.recv().await;
            log.borrow_mut().push(format!("{name} out"));
        })
        .detach();
    }
    cx.run_until_parked();
    assert_eq!(*log.borrow(), ["a in"]);
    release.try_send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(*log.borrow(), ["a in", "a out", "b in"]);
    release.try_send(()).unwrap();
    release.try_send(()).unwrap();
    cx.run_until_parked();
    assert_eq!(
        *log.borrow(),
        ["a in", "a out", "b in", "b out", "c in", "c out"]
    );
}

#[gpui::test]
fn a_signal_releases_every_waiter_now_and_later(cx: &mut TestAppContext) {
    let signal = Signal::default();
    let seen = Rc::new(Cell::new(0));
    for _ in 0..2 {
        let (signal, seen) = (signal.clone(), seen.clone());
        cx.spawn(async move |_| {
            signal.wait().await;
            seen.set(seen.get() + 1);
        })
        .detach();
    }
    cx.run_until_parked();
    assert_eq!(seen.get(), 0);
    signal.fire();
    cx.run_until_parked();
    assert_eq!(seen.get(), 2);
    let late = seen.clone();
    let again = signal.clone();
    cx.spawn(async move |_| {
        again.wait().await;
        late.set(late.get() + 1);
    })
    .detach();
    cx.run_until_parked();
    assert_eq!(seen.get(), 3);
}

#[gpui::test]
fn a_nudge_is_remembered_until_someone_waits(cx: &mut TestAppContext) {
    let nudge = Nudge::default();
    nudge.nudge();
    let seen = Rc::new(Cell::new(0));
    let (n, s) = (nudge.clone(), seen.clone());
    cx.spawn(async move |_| {
        n.wait().await;
        s.set(1);
        n.wait().await;
        s.set(2);
    })
    .detach();
    cx.run_until_parked();
    assert_eq!(seen.get(), 1, "the early nudge counted once");
    nudge.nudge();
    cx.run_until_parked();
    assert_eq!(seen.get(), 2);
}

// --- workflows ------------------------------------------------------------------------------

const WF: &str = "wf-abcd1234";

fn trigger() -> Value {
    json!({"id": "start", "type": "manual"})
}

fn session(id: &str, output: Value, extra: Value) -> Value {
    let mut node = json!({
        "id": id, "type": "session", "project": "{{inputs.repo}}",
        "prompt": format!("Do {id}"), "checkout": "root", "output": output,
    });
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

fn edge(from: &str, to: &str) -> Value {
    json!({"from": from, "to": to})
}

fn workflow(nodes: Vec<Value>, edges: Vec<Value>) -> Workflow {
    workflow_with(
        nodes,
        edges,
        json!({"repo": "/code/app", "lib": "/code/lib"}),
    )
}

fn workflow_with(nodes: Vec<Value>, edges: Vec<Value>, values: Value) -> Workflow {
    let inputs = json!({"repo": {"type": "project"}, "lib": {"type": "project"}});
    let def = json!({
        "id": WF, "name": "Ship it", "inputs": inputs, "inputValues": values,
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 0,
    });
    let built = parse_workflow(&Json::from(&def)).expect("a workflow");
    assert_eq!(validate_workflow(&built), vec![]);
    built
}

/// trigger -> dev -> report
fn simple(output: Value) -> Workflow {
    workflow(
        vec![trigger(), session("dev", output, json!({}))],
        vec![edge("start", "dev")],
    )
}

fn fenced(value: Value) -> String {
    format!("Done.\n```workflow-output\n{value}\n```")
}

fn command_workflow(extra: Value) -> Workflow {
    let mut cmd = json!({"id": "cmd", "type": "command", "project": "{{inputs.repo}}", "command": "./deploy.sh"});
    for (key, value) in extra.as_object().unwrap() {
        cmd[key] = value.clone();
    }
    workflow(vec![trigger(), cmd], vec![edge("start", "cmd")])
}

fn ask_workflow() -> Workflow {
    workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            json!({"id": "ask", "type": "switch", "decide": "ai", "question": "Ship it?", "cases": ["yes", "no"]}),
        ],
        vec![edge("start", "dev"), edge("dev", "ask")],
    )
}

fn node<'a>(run: &'a WorkflowRun, id: &str) -> &'a NodeRun {
    run.nodes.iter().find(|n| n.node_id == id).unwrap()
}

/// The declared fields of a node's output, as `name=value`; the built-ins have their own tests.
fn output_of(run: &WorkflowRun, id: &str) -> String {
    node(run, id)
        .output
        .as_ref()
        .unwrap()
        .iter()
        .filter(|(name, _)| {
            !monocode_core::mono::workflows::output::SESSION_BUILTINS
                .iter()
                .any(|(builtin, _)| builtin == name)
        })
        .map(|(k, v)| format!("{k}={}", v.stringify(false)))
        .collect::<Vec<_>>()
        .join(",")
}

fn minutes(cx: &TestAppContext, seconds: u64) {
    // The poll is every 2 s; a step this size runs every timer in between.
    cx.executor().advance_clock(Duration::from_secs(seconds));
    cx.run_until_parked();
}

// --- the run: sessions ----------------------------------------------------------------------

#[gpui::test]
fn a_run_starts_a_session_per_node_and_posts_one_report(cx: &mut TestAppContext) {
    let rig = Rig::new(&[&fenced(json!({"summary": "all good"}))]);
    let def = simple(json!({"summary": {"type": "string"}}));
    assert!(rig.start(cx, &def));
    cx.run_until_parked();

    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    let dev = node(&run, "dev");
    assert_eq!(dev.status, NodeStatus::Done);
    assert_eq!(dev.session_id.as_deref(), Some("s1"));
    assert_eq!(output_of(&run, "dev"), "summary=\"all good\"");

    let w = rig.fake.world();
    let request = &w.starts[0];
    assert_eq!(request.project, "/code/app");
    assert!(!request.new_worktree);
    assert!(request.prompt.contains("Do dev") && request.prompt.contains("app workflow.output"));
    // The session is filed in the workflow's own folder, named after it.
    assert_eq!(request.folder_name.as_deref(), Some("Ship it"));

    assert_eq!(w.posts.len(), 1);
    let post = &w.posts[0];
    assert_eq!(post.run.id, "run-1");
    assert_eq!(post.title, "Ship it · completed");
    assert!(post.text.contains("all good"));
    // The run page has the same summary.
    assert_eq!(
        run.summary
            .as_ref()
            .map(|s| (s.title.as_str(), s.text.as_str())),
        Some((post.title.as_str(), post.text.as_str()))
    );
    assert_eq!(
        post.launches,
        [SessionInfo {
            session_id: "s1".into(),
            local_id: None,
            cwd: "/code/app".into(),
            title: "Session s1".into(),
            harness: "claude".into(),
            model: Some("claude:x".into()),
        }]
    );
    drop(w);
    assert!(!rig.signals.is_running(WF), "the run is released");
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn a_second_run_of_the_same_workflow_is_refused_while_one_goes(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    assert!(rig.start(cx, &simple(json!({}))));
    cx.run_until_parked();
    assert!(rig.signals.is_running(WF));
    assert!(rig.runner.is_running(WF));
    assert!(!rig.start(cx, &simple(json!({}))), "one run per workflow");
    hold.close();
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(!rig.runner.is_running(WF));
    assert!(
        rig.start(cx, &simple(json!({}))),
        "free again once it ended"
    );
}

#[gpui::test]
fn the_record_is_written_before_anything_runs(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.saves.first().unwrap().status, RunStatus::Running);
    assert_eq!(
        node(w.saves.first().unwrap(), "dev").status,
        NodeStatus::Pending
    );
    drop(w);
    hold.close();
    cx.run_until_parked();
}

#[gpui::test]
fn it_never_asks_the_session_again_when_declared_output_is_missing(cx: &mut TestAppContext) {
    let rig = Rig::new(&["I did it.", "Never sent."]);
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert!(w.sends.is_empty(), "no follow-up in the user's session");
    assert_eq!(w.extract_calls.len(), 1, "the hidden call reads it instead");
    assert_eq!(w.posts[0].title, "Ship it \u{b7} failed");
    drop(w);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let dev = node(&run, "dev");
    assert_eq!(dev.status, NodeStatus::Failed);
    assert_eq!(dev.reasked, None);
    let error = dev.error.as_deref().unwrap();
    assert!(
        error.contains("summary") && error.contains("no extraction answer"),
        "{error}"
    );
}

#[gpui::test]
fn it_takes_what_the_session_recorded_with_workflow_output(cx: &mut TestAppContext) {
    let rig = Rig::new(&["Finished, nothing fenced."]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let values = Json::from(&json!({"summary": "from the command"}));
    let done = rig
        .runner
        .record_output(&["s1".to_string()], &values)
        .unwrap();
    assert_eq!((done.node_id.as_str(), done.round), ("dev", 1));
    assert_eq!(done.recorded, ["summary"]);
    assert!(done.missing.is_empty());
    // Another session is not a step of this run, and a value of the wrong type is refused.
    assert!(
        rig.runner
            .record_output(&["s9".to_string()], &values)
            .is_err()
    );
    let wrong = Json::from(&json!({"summary": 3}));
    assert!(
        rig.runner
            .record_output(&["s1".to_string()], &wrong)
            .is_err()
    );
    hold.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(output_of(&run, "dev"), "summary=\"from the command\"");
    assert!(rig.fake.world().sends.is_empty());
    // The run is over: nothing more can be recorded.
    assert!(
        rig.runner
            .record_output(&["s1".to_string()], &values)
            .is_err()
    );
}

#[gpui::test]
fn it_still_reads_a_block_the_session_wrote_on_its_own(cx: &mut TestAppContext) {
    let rig = Rig::new(&[&fenced(json!({"summary": "second try"}))]);
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert!(rig.fake.world().sends.is_empty());
    assert_eq!(output_of(&run, "dev"), "summary=\"second try\"");
}

#[gpui::test]
fn it_never_re_asks_a_node_that_declares_no_output(cx: &mut TestAppContext) {
    let rig = Rig::new(&["just prose, no block"]);
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(rig.fake.world().sends.is_empty());
}

#[gpui::test]
fn a_run_lets_go_of_every_session_it_used_once_it_is_over(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert!(rig.fake.world().released.is_empty(), "not while it runs");
    hold.close();
    cx.run_until_parked();
    assert_eq!(rig.fake.world().released, ["s1"]);
}

#[gpui::test]
fn a_loop_continues_the_dev_session_through_send(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), json!({})),
            session("review", json!({"verdict": {"type": "string"}}), json!({})),
        ],
        vec![
            edge("start", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "{{review.verdict}} != \"approve\""}}),
        ],
    );
    let rig = Rig::new(&[
        &fenced(json!({"summary": "v1"})),
        &fenced(json!({"verdict": "changes"})),
        &fenced(json!({"summary": "v2"})),
        &fenced(json!({"verdict": "approve"})),
    ]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 2);
    // Round 2 continues the dev session, then the review session.
    assert_eq!(
        w.sends.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
        ["s1", "s2"]
    );
    assert!(w.sends[0].1.contains("Do dev"));
    assert!(w.posts[0].text.contains("Loop rounds used: 2"));
}

#[gpui::test]
fn an_ai_decided_loop_asks_the_model_and_goes_round_again(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), json!({})),
            session("review", json!({"notes": {"type": "string"}}), json!({})),
        ],
        vec![
            edge("start", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "ai"}}),
        ],
    );
    let rig = Rig::new(&[
        &fenced(json!({"summary": "a"})),
        &fenced(json!({"notes": "meh"})),
        &fenced(json!({"summary": "b"})),
        &fenced(json!({"notes": "good"})),
    ]);
    rig.fake.answer(Ok(fenced(
        json!({"answer": "loop", "reason": "not enough"}),
    )));
    rig.fake
        .answer(Ok(fenced(json!({"answer": "exit", "reason": "good"}))));
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.decisions.len(), 2);
    assert!(w.decisions[0].prompt.contains("Allowed answers"));
    // The second round continues both sessions.
    assert_eq!(w.sends.len(), 2);
    drop(w);
    let decision = node(&run, "review").decision.clone().unwrap();
    assert_eq!(
        (decision.value.as_str(), decision.reason.as_deref()),
        ("exit", Some("good"))
    );
}

#[gpui::test]
fn the_node_fails_when_the_ais_answer_cannot_be_read(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.answer(Ok("I think so, not sure.".into()));
    rig.fake.answer(Ok("Still not sure.".into()));
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    let run = rig.last_run();
    // The step before it succeeded, so the run ends with failures rather than failed.
    assert_eq!(run.status, RunStatus::DoneWithFailures);
    let ask = node(&run, "ask");
    assert_eq!(ask.status, NodeStatus::Failed);
    assert!(ask.error.as_deref().unwrap().contains("workflow-output"));
    // A reply that cannot be read is put back once before the node fails.
    let w = rig.fake.world();
    assert_eq!(w.decisions.len(), 2);
    assert!(w.decisions[1].prompt.contains("could not be used"));
}

#[gpui::test]
fn the_ais_answer_is_a_decision_and_its_reason_is_capped(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.answer(Ok(fenced(
        json!({"answer": "yes", "reason": "r".repeat(20_000)}),
    )));
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    let decision = node(&run, "ask").decision.clone().expect("a decision");
    assert_eq!(decision.value, "yes");
    let reason = decision.reason.expect("a reason");
    assert!(reason.len() < 16_200 && reason.contains("truncated"));
}

#[gpui::test]
fn a_host_that_cannot_decide_fails_the_node_with_its_message(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.answer(Err("Codex is not installed".into()));
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    let ask = rig.last_run();
    assert_eq!(
        node(&ask, "ask").error.as_deref(),
        Some("Codex is not installed")
    );
}

#[gpui::test]
fn the_decision_is_asked_with_its_own_timeout(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.hang_next_decision();
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Running);
    // The host owns the wait (its call times out and answers with an error); the run only passes
    // the limit on.
    assert_eq!(
        rig.fake.world().decisions[0].timeout_ms,
        Limits::default().decision_timeout_ms
    );
}

// --- which model answers a decision ----------------------------------------------------------

/// What the one decision of `def` was asked with, by the model fields and the project.
fn decided_with(cx: &mut TestAppContext, def: &Workflow, replies: &[&str]) -> DecisionRequest {
    let rig = Rig::new(replies);
    rig.fake.answer(Ok(fenced(json!({"answer": "yes"}))));
    rig.start(cx, def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    assert_eq!(w.decisions.len(), 1);
    w.decisions[0].clone()
}

fn model_triple(request: &DecisionRequest) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        request.harness.as_deref(),
        request.model.as_deref(),
        request.effort.as_deref(),
    )
}

#[gpui::test]
fn the_workflows_decision_model_answers_even_when_a_session_names_another(cx: &mut TestAppContext) {
    let mut def = workflow(
        vec![
            trigger(),
            session(
                "dev",
                json!({}),
                json!({"harness": "codex", "model": "codex:test", "effort": "low"}),
            ),
            json!({"id": "ask", "type": "switch", "decide": "ai", "question": "Ship it?", "cases": ["yes", "no"]}),
        ],
        vec![edge("start", "dev"), edge("dev", "ask")],
    );
    def.decision_model = Some(DecisionModel {
        harness: "claude".into(),
        model: Some("claude:opus-5".into()),
        effort: Some("high".into()),
    });
    let request = decided_with(cx, &def, &["ok"]);
    assert_eq!(
        model_triple(&request),
        (Some("claude"), Some("claude:opus-5"), Some("high"))
    );
    assert_eq!(request.project.as_deref(), Some("/code/app"));
}

#[gpui::test]
fn a_harness_only_decision_model_asks_that_harness_with_no_model_set(cx: &mut TestAppContext) {
    let mut def = workflow(
        vec![
            trigger(),
            session(
                "dev",
                json!({}),
                json!({"harness": "codex", "model": "codex:test"}),
            ),
            json!({"id": "ask", "type": "switch", "decide": "ai", "question": "Ship it?", "cases": ["yes", "no"]}),
        ],
        vec![edge("start", "dev"), edge("dev", "ask")],
    );
    def.decision_model = Some(DecisionModel {
        harness: "claude".into(),
        model: None,
        effort: None,
    });
    // The host fills in that harness's default model when it sets the call up.
    let request = decided_with(cx, &def, &["ok"]);
    assert_eq!(model_triple(&request), (Some("claude"), None, None));
}

#[gpui::test]
fn without_a_decision_model_the_nearest_upstream_session_answers(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session(
                "far",
                json!({}),
                json!({"harness": "grok", "model": "grok:grok-4.6"}),
            ),
            session(
                "near",
                json!({}),
                json!({"harness": "codex", "model": "codex:test", "effort": "low"}),
            ),
            json!({"id": "ask", "type": "switch", "decide": "ai", "question": "Ship it?", "cases": ["yes", "no"]}),
        ],
        vec![
            edge("start", "far"),
            edge("far", "near"),
            edge("near", "ask"),
        ],
    );
    let request = decided_with(cx, &def, &["ok", "ok"]);
    assert_eq!(
        model_triple(&request),
        (Some("codex"), Some("codex:test"), Some("low"))
    );
}

#[gpui::test]
fn a_session_that_uses_the_apps_defaults_leaves_the_decision_to_them(cx: &mut TestAppContext) {
    // The session sets no harness or model, so the decision does not either: the host answers
    // with the app's default for a new session, in the project the session works in.
    let request = decided_with(cx, &ask_workflow(), &["ok"]);
    assert_eq!(model_triple(&request), (None, None, None));
    assert_eq!(request.project.as_deref(), Some("/code/app"));
    assert!(request.prompt.contains("Ship it?"));
}

#[gpui::test]
fn a_loop_edge_decides_on_the_model_of_the_session_it_leaves(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), json!({})),
            session(
                "review",
                json!({"summary": {"type": "string"}}),
                json!({"harness": "codex", "model": "codex:test"}),
            ),
        ],
        vec![
            edge("start", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 2, "decide": "ai"}}),
        ],
    );
    let rig = Rig::new(&[
        &fenced(json!({"summary": "a"})),
        &fenced(json!({"summary": "b"})),
    ]);
    rig.fake
        .answer(Ok(fenced(json!({"answer": "exit", "reason": "done"}))));
    rig.start(cx, &def);
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.decisions.len(), 1);
    assert_eq!(
        model_triple(&w.decisions[0]),
        (Some("codex"), Some("codex:test"), None)
    );
}

#[gpui::test]
fn a_long_output_string_is_cut_before_it_is_stored(cx: &mut TestAppContext) {
    let rig = Rig::new(&[&fenced(json!({"summary": "x".repeat(20_000)}))]);
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let run = rig.last_run();
    let output = output_of(&run, "dev");
    assert!(output.len() < 16_200 && output.contains("truncated"));
}

#[gpui::test]
fn a_project_that_is_not_an_app_project_fails_the_node(cx: &mut TestAppContext) {
    let def = workflow_with(
        vec![
            trigger(),
            session("dev", json!({}), json!({"project": "{{inputs.lib}}"})),
        ],
        vec![edge("start", "dev")],
        json!({"repo": "/code/app", "lib": "/elsewhere"}),
    );
    let rig = Rig::new(&[]);
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("\"{{inputs.lib}}\" is not one of the app's projects")
    );
    assert!(rig.fake.world().starts.is_empty());
}

#[gpui::test]
fn a_session_that_fails_to_start_fails_its_node_with_the_hosts_message(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .fail_starts("Model \"codex:gone\" is not available. Pick one in the step's model list.");
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        node(&run, "dev")
            .error
            .as_deref()
            .unwrap()
            .starts_with("Model \"codex:gone\"")
    );
}

#[gpui::test]
fn a_session_that_ends_in_error_fails_its_node_with_the_error(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .end_turns_with(Outcome::failed("Claude Code reached its usage limit."));
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("Claude Code reached its usage limit.")
    );
}

#[gpui::test]
fn it_nests_under_another_nodes_session_and_uses_its_worktree(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            session(
                "review",
                json!({}),
                json!({"nestUnder": "dev", "checkout": {"worktreeOf": "dev"}}),
            ),
        ],
        vec![edge("start", "dev"), edge("dev", "review")],
    );
    let rig = Rig::new(&["ok", "ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    let second = &w.starts[1];
    assert!(!second.new_worktree);
    assert_eq!(second.parent_session_id.as_deref(), Some("s1"));
    assert_eq!(second.worktree_cwd.as_deref(), Some("/code/app-wt-s1"));
    assert_eq!(second.folder_name, w.starts[0].folder_name);
}

#[gpui::test]
fn a_new_worktree_checkout_asks_for_one(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session(
                "dev",
                json!({}),
                json!({"checkout": "newWorktree", "harness": "codex", "model": "codex:test", "effort": "high"}),
            ),
        ],
        vec![edge("start", "dev")],
    );
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    let w = rig.fake.world();
    let request = &w.starts[0];
    assert!(request.new_worktree);
    assert_eq!(
        (
            request.harness.as_deref(),
            request.model.as_deref(),
            request.effort.as_deref()
        ),
        (Some("codex"), Some("codex:test"), Some("high"))
    );
}

#[gpui::test]
fn it_fails_the_node_when_the_worktree_it_wants_is_not_there(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            session(
                "review",
                json!({}),
                json!({"checkout": {"worktreeOf": "dev"}}),
            ),
        ],
        vec![edge("start", "dev"), edge("dev", "review")],
    );
    let rig = Rig::new(&["ok"]);
    rig.fake.no_worktrees();
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(
        node(&run, "review").error.as_deref(),
        Some("No worktree is available from \"dev\"")
    );
    assert_eq!(rig.fake.world().starts.len(), 1);
}

#[gpui::test]
fn every_session_of_a_workflow_is_filed_in_the_folder_named_after_it(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            session("docs", json!({}), json!({"project": "{{inputs.lib}}"})),
            session("more", json!({}), json!({"project": "{{inputs.lib}}"})),
        ],
        vec![
            edge("start", "dev"),
            edge("dev", "docs"),
            edge("docs", "more"),
        ],
    );
    let rig = Rig::new(&["ok", "ok", "ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    let w = rig.fake.world();
    // The folder is the workflow's, whatever the project and the run: the host finds it by name
    // in the project's folders or makes it.
    let folders: Vec<_> = w.starts.iter().map(|s| s.folder_name.as_deref()).collect();
    assert_eq!(folders, [Some("Ship it"), Some("Ship it"), Some("Ship it")]);
    assert_eq!(w.starts[0].project, "/code/app");
    assert_eq!(w.starts[1].project, "/code/lib");
}

// --- the run: timeouts and approvals --------------------------------------------------------

fn one_minute() -> Workflow {
    workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({"timeoutMinutes": 1})),
        ],
        vec![edge("start", "dev")],
    )
}

#[gpui::test]
fn a_session_that_works_too_long_is_stopped_and_fails(cx: &mut TestAppContext) {
    let rig = Rig::new(&["late"]);
    rig.fake.hold();
    rig.start(cx, &one_minute());
    cx.run_until_parked();
    minutes(cx, 58);
    assert_eq!(rig.last_run().status, RunStatus::Running);
    assert!(rig.fake.world().stops.is_empty());
    minutes(cx, 4);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("It took longer than 1 minutes and was stopped.")
    );
    assert_eq!(rig.fake.world().stops, ["s1"]);
}

#[gpui::test]
fn time_spent_waiting_for_an_approval_does_not_count_toward_the_timeout(cx: &mut TestAppContext) {
    let rig = Rig::new(&["done"]);
    let hold = rig.fake.hold();
    rig.start(cx, &one_minute());
    cx.run_until_parked();
    minutes(cx, 20);
    // Five minutes on the user: far past the node's minute.
    rig.fake.set_waiting("s1", true);
    minutes(cx, 5 * 60);
    assert_eq!(rig.last_run().status, RunStatus::Running);
    assert_eq!(
        node(&rig.last_run(), "dev").status,
        NodeStatus::WaitingApproval
    );
    rig.fake.set_waiting("s1", false);
    minutes(cx, 30);
    assert_eq!(node(&rig.last_run(), "dev").status, NodeStatus::Running);
    hold.close();
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(rig.fake.world().stops.is_empty());
}

#[gpui::test]
fn working_time_before_and_after_an_approval_adds_up(cx: &mut TestAppContext) {
    let rig = Rig::new(&["done"]);
    rig.fake.hold();
    rig.start(cx, &one_minute());
    cx.run_until_parked();
    minutes(cx, 40);
    rig.fake.set_waiting("s1", true);
    minutes(cx, 600);
    rig.fake.set_waiting("s1", false);
    minutes(cx, 16);
    assert_eq!(rig.last_run().status, RunStatus::Running);
    minutes(cx, 6);
    assert_eq!(rig.last_run().status, RunStatus::Failed);
}

#[gpui::test]
fn an_approval_nobody_answers_ends_the_session(cx: &mut TestAppContext) {
    let rig = Rig::with_limits(
        &[],
        Limits {
            approval_wait_ms: 10_000,
            ..Limits::default()
        },
    );
    rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.fake.set_waiting("s1", true);
    minutes(cx, 12);
    let run = rig.last_run();
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("Nobody answered its approval request.")
    );
    assert_eq!(rig.fake.world().stops, ["s1"]);
}

#[gpui::test]
fn an_approval_marks_the_node_waiting_and_no_mono_is_asked(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.fake.set_waiting("s1", true);
    minutes(cx, 5);
    // The session shows the approval itself (the app raises its usual notices); the run only
    // marks the node.
    assert_eq!(
        node(&rig.last_run(), "dev").status,
        NodeStatus::WaitingApproval
    );

    rig.fake.set_waiting("s1", false);
    hold.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
}

// --- the run: commands ----------------------------------------------------------------------

#[gpui::test]
fn a_command_runs_in_the_project_and_its_output_is_kept(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .command_returns(command_result(Some(0), "fine\n", "warn\n"));
    rig.start(
        cx,
        &command_workflow(json!({"command": "echo hi", "timeoutMinutes": 2})),
    );
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(
        output_of(&run, "cmd"),
        "exitCode=0,stdout=\"fine\\n\",stderr=\"warn\\n\""
    );
    let w = rig.fake.world();
    let (cwd, command, timeout, id) = w.commands[0].clone();
    assert_eq!(
        (cwd.as_str(), command.as_str(), timeout),
        ("/code/app", "echo hi", 2 * 60_000)
    );
    let parts: Vec<&str> = id.split(':').collect();
    assert_eq!(
        (parts[0], parts[2], parts[3], parts[4]),
        ("run-1", "cmd", "", "1")
    );
}

#[gpui::test]
fn a_command_runs_in_the_worktree_of_an_earlier_session(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            json!({"id": "cmd", "type": "command", "project": "{{inputs.repo}}", "command": "make", "cwd": {"worktreeOf": "dev"}}),
        ],
        vec![edge("start", "dev"), edge("dev", "cmd")],
    );
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.fake.world().commands[0].0, "/code/app-wt-s1");
    assert_eq!(rig.last_run().status, RunStatus::Done);
}

#[gpui::test]
fn a_failing_command_fails_only_its_branch(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("fetch", json!({"tickets": {"type": "list"}}), json!({})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"}),
            json!({"id": "cmd", "type": "command", "project": "{{inputs.repo}}", "command": "echo {{item}}", "parent": "each"}),
        ],
        vec![edge("start", "fetch"), edge("fetch", "each")],
    );
    let rig = Rig::new(&[&fenced(json!({"tickets": ["ok", "bad"]}))]);
    rig.fake
        .command_returns(command_result(Some(0), "fine", ""));
    rig.fake
        .command_returns(command_result(Some(1), "", "boom"));
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::DoneWithFailures);
    let cmds: Vec<_> = run.nodes.iter().filter(|n| n.node_id == "cmd").collect();
    assert_eq!(
        cmds.iter()
            .map(|n| (n.branch.as_str(), n.status))
            .collect::<Vec<_>>(),
        [("each#0", NodeStatus::Done), ("each#1", NodeStatus::Failed)]
    );
    assert_eq!(
        cmds[1].error.as_deref(),
        Some("The command exited with code 1: boom")
    );
    let w = rig.fake.world();
    assert_eq!(w.commands[0].1, "echo 'ok'");
    assert_eq!(w.commands[0].2, 10 * 60_000);
    let ids: std::collections::HashSet<_> = w.commands.iter().map(|c| c.3.clone()).collect();
    assert_eq!(ids.len(), w.commands.len(), "every command has its own id");
    assert_eq!(w.posts[0].title, "Ship it · completed with failures");
}

#[gpui::test]
fn a_command_that_exits_without_a_code_or_output_says_so(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.command_returns(command_result(None, "", ""));
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    assert_eq!(
        node(&rig.last_run(), "cmd").error.as_deref(),
        Some("The command exited with code unknown")
    );
}

#[gpui::test]
fn a_failed_command_names_the_end_of_its_output(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let stdout = format!("{}TAIL", "o".repeat(900));
    rig.fake
        .command_returns(command_result(Some(2), &stdout, "  \n"));
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    let error = node(&rig.last_run(), "cmd").error.clone().unwrap();
    // stderr is blank, so stdout speaks, cut to its last 500 characters.
    assert!(
        error.starts_with("The command exited with code 2: ooo") && error.ends_with("oTAIL"),
        "{error}"
    );
    assert_eq!(error.len(), "The command exited with code 2: ".len() + 500);
}

#[gpui::test]
fn a_command_that_times_out_fails_its_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let mut result = command_result(None, "", "");
    result.timed_out = true;
    rig.fake.command_returns(result);
    rig.start(cx, &command_workflow(json!({"timeoutMinutes": 2})));
    cx.run_until_parked();
    assert_eq!(rig.fake.world().commands[0].2, 2 * 60_000);
    assert_eq!(
        node(&rig.last_run(), "cmd").error.as_deref(),
        Some("The command timed out after 2 minutes")
    );
}

#[gpui::test]
fn a_command_cancelled_from_outside_fails_its_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let mut result = command_result(None, "", "");
    result.cancelled = true;
    rig.fake.command_returns(result);
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        node(&run, "cmd")
            .error
            .as_deref()
            .unwrap()
            .contains("cancelled")
    );
}

#[gpui::test]
fn a_command_the_host_refuses_fails_with_the_hosts_message(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake
        .command_fails("Command nodes are not supported on Windows yet");
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    assert!(
        node(&rig.last_run(), "cmd")
            .error
            .as_deref()
            .unwrap()
            .contains("Command nodes are not supported on Windows yet")
    );
}

// --- stop -----------------------------------------------------------------------------------

#[gpui::test]
fn stop_stops_running_sessions_ends_stopped_and_reports_once(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert_eq!(
        node(&rig.last_run(), "dev").session_id.as_deref(),
        Some("s1")
    );

    assert!(rig.runner.stop(WF, None));
    assert!(!rig.runner.stop(WF, None), "a second stop changes nothing");
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(node(&run, "dev").status, NodeStatus::Stopped);
    let w = rig.fake.world();
    assert_eq!(w.stops, ["s1"]);
    assert_eq!(w.posts.len(), 1, "a stopped run reports once");
    assert_eq!(w.posts[0].title, "Ship it · stopped");
    assert!(w.posts[0].text.contains("stopped"));
    drop(w);
    assert!(!rig.signals.is_running(WF));
    assert!(run.report_posted_at.is_some());
}

#[gpui::test]
fn stop_cancels_a_running_command_and_the_run_is_released_after_it_settles(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.command_gate();
    let mut result = command_result(None, "partial", "");
    result.cancelled = true;
    rig.fake.command_returns(result);
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    let id = rig.fake.world().commands[0].3.clone();

    rig.runner.stop(WF, None);
    cx.run_until_parked();
    assert!(rig.fake.world().cancelled.contains(&id));
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(node(&run, "cmd").status, NodeStatus::Stopped);
    // The command is still alive: another run of the workflow must not start yet.
    minutes(cx, 1);
    assert!(rig.runner.is_running(WF));
    assert!(rig.signals.is_running(WF));

    gate.close();
    cx.run_until_parked();
    assert!(!rig.runner.is_running(WF));
    assert_eq!(rig.fake.world().posts.len(), 1);
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
}

#[gpui::test]
fn stop_retries_a_cancel_that_beat_the_command_starting(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.command_gate();
    rig.fake.command_returns(command_result(None, "", ""));
    rig.fake.cancel_refuses(3);
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    rig.runner.stop(WF, None);
    cx.run_until_parked();
    minutes(cx, 1);
    // Three misses, then it took (the run asks once more when it is over).
    assert!(rig.fake.world().cancelled.len() >= 4);
    gate.close();
    cx.run_until_parked();
}

#[gpui::test]
fn stop_does_not_wait_for_a_pending_decision(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.hang_next_decision();
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    assert_eq!(rig.fake.world().decisions.len(), 1);
    rig.runner.stop(WF, None);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn stop_does_not_wait_for_a_session_that_never_finishes_starting(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let gate = rig.fake.start_gate();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert_eq!(rig.fake.world().starts.len(), 1);
    rig.runner.stop(WF, None);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    assert!(!rig.runner.is_running(WF));
    assert!(rig.fake.world().stops.is_empty());
    // A start that finally comes through after the stop is still ended.
    gate.close();
    cx.run_until_parked();
    assert_eq!(rig.fake.world().stops, ["s1"]);
    // And the host lets go of it, so it keeps no watch, wake-up or id for a run that is over.
    assert_eq!(rig.fake.world().released, ["s1"]);
}

#[gpui::test]
fn starts_queued_behind_another_are_not_launched_once_the_run_is_stopped(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("fetch", json!({"tickets": {"type": "list"}}), json!({})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"}),
            session("work", json!({}), json!({"parent": "each"})),
        ],
        vec![edge("start", "fetch"), edge("fetch", "each")],
    );
    let rig = Rig::new(&[&fenced(json!({"tickets": ["a", "b"]}))]);
    // The first branch's start is slow, so the second one waits in the queue behind it.
    let gate = rig.fake.start_gate_for(2);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert_eq!(rig.fake.world().starts.len(), 2);

    rig.runner.stop(WF, None);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    gate.close();
    cx.run_until_parked();
    minutes(cx, 1);
    let w = rig.fake.world();
    // fetch and the first branch only; the first branch's session is stopped as it appears.
    assert_eq!(w.starts.len(), 2);
    assert_eq!(w.stops, ["s2"]);
    drop(w);
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn stop_goes_on_when_a_session_cannot_be_stopped(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.hold();
    rig.fake.stop_fails();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.runner.stop(WF, None);
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
    let w = rig.fake.world();
    assert_eq!(w.posts.len(), 1);
}

#[gpui::test]
fn stopping_a_run_that_is_over_does_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    assert!(!rig.runner.stop(WF, None));
    assert!(!rig.runner.stop("wf-unknown1", None));
    assert_eq!(rig.last_run().status, RunStatus::Done);
}

#[gpui::test]
fn stopping_a_run_that_is_already_over_does_not_hold_it_open(cx: &mut TestAppContext) {
    let def = simple(json!({}));
    let mut run = start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    run.status = RunStatus::Failed;
    run.ended_at = Some(2_000);
    let rig = Rig::new(&[]);
    rig.start_run(cx, run);
    // Before the first save has even run.
    assert!(!rig.runner.stop(WF, None));
    cx.run_until_parked();
    assert!(!rig.runner.is_running(WF));
    assert!(!rig.signals.is_running(WF));
}

// --- persistence ----------------------------------------------------------------------------

#[gpui::test]
fn it_keeps_going_in_memory_when_the_run_cannot_be_saved(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.fail_saves("file too large");
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.posts.len(), 1);
    // The report mentions that the history could not be written.
    assert!(w.posts[0].text.contains("Run error"), "{}", w.posts[0].text);
    assert!(
        w.posts[0]
            .text
            .contains("Could not save the run history: file too large")
    );
    drop(w);
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn saves_fold_into_the_newest_state_and_never_overlap(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.slow_saves(20);
    rig.start(cx, &simple(json!({})));
    for _ in 0..40 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(10));
    }
    assert!(!rig.runner.is_running(WF));
    let w = rig.fake.world();
    assert_eq!(w.max_inflight, 1);
    // Start, node started, then everything that happened while that write was going.
    assert!(w.saves.len() > 1 && w.saves.len() <= 4, "{}", w.saves.len());
    let last = w.saves.last().unwrap();
    assert_eq!(last.status, RunStatus::Done);
    assert_eq!(node(last, "dev").status, NodeStatus::Done);
    assert_eq!(node(last, "dev").session_id.as_deref(), Some("s1"));
}

#[gpui::test]
fn a_run_that_cannot_make_progress_fails_saying_so(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::engine::{EngineEvent, apply_event};
    let def = simple(json!({}));
    let started = start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    // A run left `running` with its node running and no task behind it.
    let stuck = apply_event(
        &started,
        &EngineEvent::NodeStarted {
            node_id: "dev".into(),
            branch: String::new(),
            session_id: None,
            at: 2_000,
        },
    );
    assert_eq!(node(&stuck, "dev").status, NodeStatus::Running);
    let rig = Rig::new(&[]);
    assert!(rig.start_run(cx, stuck));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        run.error.as_deref(),
        Some("The run could not make progress.")
    );
    assert!(run.ended_at.is_some());
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn a_run_that_is_already_over_is_only_written(cx: &mut TestAppContext) {
    let def = simple(json!({}));
    let mut run = start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    run.status = RunStatus::Failed;
    run.ended_at = Some(2_000);
    run.error = Some("The workflow is not valid: nope".into());
    // Its summary went out when it was made, so nothing is delivered again.
    run.report_posted_at = Some(2_000);
    let rig = Rig::new(&[]);
    rig.start_run(cx, run);
    cx.run_until_parked();
    assert_eq!(
        rig.last_run().error.as_deref(),
        Some("The workflow is not valid: nope")
    );
    let w = rig.fake.world();
    assert!(w.starts.is_empty() && w.posts.is_empty());
    drop(w);
    assert!(!rig.runner.is_running(WF));
}

// --- resume ---------------------------------------------------------------------------------

#[gpui::test]
fn it_resumes_an_interrupted_run_from_where_it_stopped(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::engine::mark_interrupted;
    let def = simple(json!({}));
    let interrupted = mark_interrupted(&start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-old",
    ));
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    let rig = Rig::new(&["ok"]);
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(rig.runner.resume(host, &cx.to_async(), &interrupted));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!((run.id.as_str(), run.status), ("run-old", RunStatus::Done));
    let w = rig.fake.world();
    assert_eq!(w.starts.len(), 1);
    assert_eq!(w.posts.len(), 1);
}

#[gpui::test]
fn only_an_interrupted_run_can_be_resumed(cx: &mut TestAppContext) {
    let def = simple(json!({}));
    let running = start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    let rig = Rig::new(&[]);
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(!rig.runner.resume(host, &cx.to_async(), &running));
    cx.run_until_parked();
    assert!(rig.fake.world().saves.is_empty());
}

#[gpui::test]
fn a_resumed_loop_node_continues_its_session_with_a_follow_up(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::engine::{EngineEvent, apply_event, mark_interrupted};
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({"summary": {"type": "string"}}), json!({})),
            session("review", json!({"verdict": {"type": "string"}}), json!({})),
        ],
        vec![
            edge("start", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "{{review.verdict}} != \"approve\""}}),
        ],
    );
    // Round 1 happened; round 2's dev was running in session s9 when the app closed.
    let mut run = start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-old",
    );
    let at = 2_000;
    let step = |run: &WorkflowRun, event| apply_event(run, &event);
    run = step(
        &run,
        EngineEvent::NodeStarted {
            node_id: "dev".into(),
            branch: String::new(),
            session_id: None,
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeFinished {
            node_id: "dev".into(),
            branch: String::new(),
            output: out(json!({"summary": "v1"})),
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeStarted {
            node_id: "review".into(),
            branch: String::new(),
            session_id: None,
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeFinished {
            node_id: "review".into(),
            branch: String::new(),
            output: out(json!({"verdict": "changes"})),
            at,
        },
    );
    run = step(
        &run,
        EngineEvent::NodeStarted {
            node_id: "dev".into(),
            branch: String::new(),
            session_id: Some("s9".into()),
            at,
        },
    );
    run.nodes
        .iter_mut()
        .find(|n| n.node_id == "dev")
        .unwrap()
        .session_id = Some("s9".into());
    let interrupted = mark_interrupted(&run);
    assert_eq!(
        node(&interrupted, "dev").reuse_session_id.as_deref(),
        Some("s9")
    );

    let rig = Rig::new(&[
        &fenced(json!({"summary": "v2"})),
        &fenced(json!({"verdict": "approve"})),
    ]);
    let host: Rc<dyn WorkflowHost> = Rc::new(rig.fake.clone());
    assert!(rig.runner.resume(host, &cx.to_async(), &interrupted));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(
        w.sends[0].0, "s9",
        "the loop session is continued, not started again"
    );
    assert_eq!(w.starts.len(), 1, "only review starts a session of its own");
    drop(w);
    assert_eq!(rig.last_run().status, RunStatus::Done);
}

fn out(value: Value) -> OrderedMap<Json> {
    match Json::from(&value) {
        Json::Object(map) => map,
        _ => panic!("not an object"),
    }
}

fn command_result(code: Option<i32>, stdout: &str, stderr: &str) -> CommandResult {
    CommandResult {
        exit_code: code,
        stdout: stdout.into(),
        stderr: stderr.into(),
        timed_out: false,
        cancelled: false,
    }
}

// --- quitting, deleting a Mono ----------------------------------------------

#[gpui::test]
fn quitting_leaves_a_run_running_in_the_file_for_the_next_start_to_interrupt(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(&["ok"]);
    let hold = rig.fake.hold();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let saved = rig.fake.world().saves.len();
    rig.runner.quit();
    hold.close();
    cx.run_until_parked();
    // What the sessions report while the app goes down changes nothing: the file still says
    // `running`, no stop report was posted and nothing was stopped on the Mono's behalf.
    assert_eq!(rig.last_run().status, RunStatus::Running);
    assert_eq!(rig.fake.world().saves.len(), saved);
    assert!(rig.fake.world().posts.is_empty());
    assert!(rig.fake.world().stops.is_empty());
    assert!(!rig.runner.is_running(WF));
    assert!(!rig.signals.is_running(WF));
    // A second quit, and a stop after it, do nothing.
    rig.runner.quit();
    assert!(!rig.runner.stop(WF, None));
}

#[gpui::test]
fn a_command_that_dies_with_the_app_does_not_fail_its_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.command_gate();
    rig.fake.command_returns(command_result(None, "", "killed"));
    rig.start(cx, &command_workflow(json!({})));
    cx.run_until_parked();
    rig.runner.quit();
    gate.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_ne!(node(&run, "cmd").status, NodeStatus::Failed);
    assert!(rig.fake.world().posts.is_empty());
}

#[gpui::test]
fn a_session_that_finishes_starting_after_the_quit_writes_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    let gate = rig.fake.start_gate();
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let saved = rig.fake.world().saves.len();
    rig.runner.quit();
    gate.close();
    cx.run_until_parked();
    // The start came through (with its folder) after the app began to go down: not recorded.
    assert_eq!(rig.fake.world().saves.len(), saved);
    assert_eq!(rig.last_run().status, RunStatus::Running);
}

#[gpui::test]
fn a_session_that_fails_to_start_during_the_quit_does_not_fail_the_node(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    let gate = rig.fake.start_gate();
    rig.fake.fail_starts("The app is closing");
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    rig.runner.quit();
    gate.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_ne!(node(&run, "dev").status, NodeStatus::Failed);
}

#[gpui::test]
fn nothing_starts_or_resumes_once_the_app_is_quitting(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.runner.quit();
    assert!(!rig.start(cx, &simple(json!({}))));
    let interrupted = monocode_core::mono::workflows::engine::mark_interrupted(&start_run(
        &simple(json!({})),
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-old",
    ));
    assert!(
        !rig.runner
            .resume(Rc::new(rig.fake.clone()), &cx.to_async(), &interrupted)
    );
    cx.run_until_parked();
    assert!(rig.fake.world().saves.is_empty(), "not even a record");
    assert!(!rig.signals.is_running(WF));
}

#[gpui::test]
fn quitting_stops_waiting_on_a_decision(cx: &mut TestAppContext) {
    let rig = Rig::new(&[]);
    rig.fake.hang_next_decision();
    rig.start(cx, &ask_workflow());
    cx.run_until_parked();
    rig.runner.quit();
    cx.run_until_parked();
    assert!(!rig.runner.is_running(WF));
    assert_eq!(rig.last_run().status, RunStatus::Running);
}
