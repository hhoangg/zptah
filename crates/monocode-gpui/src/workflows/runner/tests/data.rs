//! Data passing against the fake host: the built-in outputs, the fallback extraction that reads
//! what a step did not record, the hint in the first prompt, and "Wait for me".
use super::*;
use monocode_core::mono::workflows::session_facts::SessionFacts;

fn value(run: &WorkflowRun, id: &str, key: &str) -> Option<Json> {
    node(run, id)
        .output
        .as_ref()
        .and_then(|output| output.get(key).cloned())
}

fn text(run: &WorkflowRun, id: &str, key: &str) -> String {
    value(run, id, key)
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn keys(run: &WorkflowRun, id: &str) -> Vec<String> {
    node(run, id)
        .output
        .as_ref()
        .map(|output| output.keys().cloned().collect())
        .unwrap_or_default()
}

fn record(rig: &Rig, session: &str, values: Value) -> Result<RecordedOutput, String> {
    rig.runner
        .record_output(&[session.to_string()], &Json::from(&values))
}

fn answer(rig: &Rig, session: &str, choice: WaitChoice) -> Result<(), String> {
    rig.runner.answer_wait(&[session.to_string()], choice)
}

// --- built-in outputs -----------------------------------------------------------------------

#[gpui::test]
fn a_chat_step_gets_every_built_in_output(cx: &mut TestAppContext) {
    let rig = Rig::new(&["All done."]);
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(text(&run, "dev", "reply"), "All done.");
    assert_eq!(text(&run, "dev", "sessionId"), "s1");
    assert_eq!(text(&run, "dev", "worktree"), "/code/app-wt-s1");
    assert_eq!(text(&run, "dev", "branch"), "feature");
    assert_eq!(text(&run, "dev", "commit"), "abc123");
    assert_eq!(text(&run, "dev", "diffStat"), " src/a.rs | 1 +");
    assert_eq!(
        value(&run, "dev", "changedFiles").map(|v| v.stringify(false)),
        Some("[\"src/a.rs\"]".to_string())
    );
    // The changes are measured from the commit the step started at.
    let w = rig.fake.world();
    assert_eq!(w.head_calls, ["/code/app"]);
    assert_eq!(
        w.fact_calls,
        [("s1".to_string(), Some("base0".to_string()))]
    );
}

#[gpui::test]
fn the_base_of_a_followup_round_is_the_head_when_that_round_starts(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("dev", json!({}), json!({})),
            session("review", json!({"verdict": {"type": "string"}}), json!({})),
        ],
        vec![
            edge("start", "dev"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 2, "decide": "{{review.verdict}} != \"approve\""}}),
        ],
    );
    let rig = Rig::new(&[
        "one",
        &fenced(json!({"verdict": "changes"})),
        "two",
        &fenced(json!({"verdict": "approve"})),
    ]);
    rig.start(cx, &def);
    cx.run_until_parked();
    let w = rig.fake.world();
    // Round 1 asks the project, round 2 the worktree of the session it continues.
    assert_eq!(w.head_calls[0], "/code/app");
    assert!(w.head_calls.iter().any(|dir| dir == "/code/app-wt-s1"));
}

#[gpui::test]
fn a_terminal_step_has_no_reply_but_the_rest(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ignored"]);
    let def = workflow(
        vec![
            trigger(),
            session("ship", json!({}), json!({"surface": "terminal"})),
        ],
        vec![edge("start", "ship")],
    );
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert!(rig.fake.world().starts[0].terminal);
    assert_eq!(
        keys(&run, "ship"),
        [
            "sessionId",
            "worktree",
            "branch",
            "commit",
            "changedFiles",
            "diffStat"
        ]
    );
}

#[gpui::test]
fn a_cloud_step_has_only_reply_and_session_id(cx: &mut TestAppContext) {
    let rig = Rig::new(&["From the cloud."]);
    let def = workflow(
        vec![
            trigger(),
            session(
                "dev",
                json!({}),
                json!({"target": {"kind": "codespace", "branch": "trunk"}}),
            ),
        ],
        vec![edge("start", "dev")],
    );
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(keys(&run, "dev"), ["reply", "sessionId"]);
    assert_eq!(text(&run, "dev", "reply"), "From the cloud.");
    let w = rig.fake.world();
    assert!(
        w.head_calls.is_empty() && w.fact_calls.is_empty(),
        "no git in the cloud"
    );
}

#[gpui::test]
fn a_checkout_without_a_repository_gives_empty_git_values(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake.head_is(None);
    rig.fake.facts_are(SessionFacts {
        worktree: "/code/app".into(),
        ..SessionFacts::default()
    });
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(text(&run, "dev", "commit"), "");
    assert_eq!(text(&run, "dev", "diffStat"), "");
    assert_eq!(
        value(&run, "dev", "changedFiles").map(|v| v.stringify(false)),
        Some("[]".to_string())
    );
    assert_eq!(rig.fake.world().fact_calls[0].1, None, "no base was known");
}

// --- fallback extraction --------------------------------------------------------------------

fn two_fields() -> Workflow {
    simple(json!({"a": {"type": "string"}, "b": {"type": "string"}}))
}

#[gpui::test]
fn extraction_fills_only_the_fields_the_session_did_not_record(cx: &mut TestAppContext) {
    let rig = Rig::new(&["Done, but I never ran the command for b."]);
    // The hidden call also answers for `a`; what the session recorded stays.
    rig.fake.extraction(Ok(fenced(
        json!({"a": "from extraction", "b": "read from the chat"}),
    )));
    let hold = rig.fake.hold();
    rig.start(cx, &two_fields());
    cx.run_until_parked();
    record(&rig, "s1", json!({"a": "from the command"})).unwrap();
    hold.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(
        output_of(&run, "dev"),
        "a=\"from the command\",b=\"read from the chat\""
    );
    let w = rig.fake.world();
    assert_eq!(w.extract_calls.len(), 1);
    // It speaks for this very session, about the missing field only, and asks nothing of it.
    assert_eq!(w.extract_calls[0].session_id, "s1");
    assert_eq!(
        w.extract_calls[0]
            .fields
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        ["b"]
    );
    assert!(w.sends.is_empty(), "never a message in the user's session");
}

#[gpui::test]
fn nothing_is_extracted_when_every_field_is_there(cx: &mut TestAppContext) {
    let rig = Rig::new(&[&fenced(json!({"a": "x", "b": "y"}))]);
    rig.start(cx, &two_fields());
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
    assert!(rig.fake.world().extract_calls.is_empty());
}

#[gpui::test]
fn a_failed_extraction_fails_the_step_with_the_reason(cx: &mut TestAppContext) {
    let rig = Rig::new(&["No fields in sight."]);
    rig.fake
        .extraction(Err("The AI did not answer in time".into()));
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let error = node(&run, "dev").error.clone().unwrap();
    assert!(
        error.contains("summary") && error.contains("did not answer in time"),
        "{error}"
    );
    assert!(rig.fake.world().sends.is_empty());
}

#[gpui::test]
fn an_extraction_answer_of_the_wrong_type_fails_the_step(cx: &mut TestAppContext) {
    let rig = Rig::new(&["No fields in sight."]);
    rig.fake.extraction(Ok(fenced(json!({"summary": 3}))));
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let error = node(&run, "dev").error.clone().unwrap();
    assert!(
        error.contains("summary") && error.contains("string"),
        "{error}"
    );
}

#[gpui::test]
fn an_extraction_answer_without_the_block_fails_the_step(cx: &mut TestAppContext) {
    let rig = Rig::new(&["No fields in sight."]);
    rig.fake.extraction(Ok("I could not tell.".into()));
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Failed);
}

#[gpui::test]
fn a_terminal_step_with_a_field_missing_fails_and_is_not_extracted(cx: &mut TestAppContext) {
    let rig = Rig::new(&["ok"]);
    rig.fake
        .extraction(Ok(fenced(json!({"summary": "never used"}))));
    let def = workflow(
        vec![
            trigger(),
            session(
                "ship",
                json!({"summary": {"type": "string"}}),
                json!({"surface": "terminal"}),
            ),
        ],
        vec![edge("start", "ship")],
    );
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let error = node(&run, "ship").error.clone().unwrap();
    assert!(
        error.contains("terminal session")
            && error.contains("workflow.output")
            && error.contains("summary"),
        "{error}"
    );
    assert!(rig.fake.world().extract_calls.is_empty());
}

#[gpui::test]
fn a_stop_does_not_wait_for_an_extraction_that_hangs(cx: &mut TestAppContext) {
    let rig = Rig::new(&["No fields in sight."]);
    rig.fake.hang_next_extraction();
    rig.start(cx, &simple(json!({"summary": {"type": "string"}})));
    cx.run_until_parked();
    assert!(rig.runner.is_running(WF), "the run waits for the answer");
    assert!(rig.runner.stop(WF, None));
    cx.run_until_parked();
    assert!(!rig.runner.is_running(WF));
    assert_eq!(rig.last_run().status, RunStatus::Stopped);
}

// --- the first prompt -----------------------------------------------------------------------

#[gpui::test]
fn the_hint_is_in_the_first_prompt_only(cx: &mut TestAppContext) {
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
    let w = rig.fake.world();
    assert!(w.starts[0].prompt.contains("app workflow.output"));
    assert!(w.starts[1].prompt.contains("app workflow.output"));
    assert_eq!(w.sends.len(), 2, "round 2 continues both sessions");
    for (_, prompt) in &w.sends {
        assert!(!prompt.contains("workflow.output"), "{prompt}");
    }
    assert!(w.sends[0].1.contains("Do dev"));
}

#[gpui::test]
fn a_cloud_step_gets_no_hint(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session(
                "dev",
                json!({"summary": {"type": "string"}}),
                json!({"target": {"kind": "codespace"}}),
            ),
        ],
        vec![edge("start", "dev")],
    );
    let rig = Rig::new(&[&fenced(json!({"summary": "x"}))]);
    rig.start(cx, &def);
    cx.run_until_parked();
    assert!(
        !rig.fake.world().starts[0]
            .prompt
            .contains("workflow.output")
    );
}

// --- Wait for me ----------------------------------------------------------------------------

fn waiting(output: Value) -> Workflow {
    workflow(
        vec![
            trigger(),
            session("dev", output, json!({"mode": "waitForMe"})),
        ],
        vec![edge("start", "dev")],
    )
}

#[gpui::test]
fn a_wait_for_me_step_waits_after_its_first_turn_and_continue_settles_it_once(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(&["First answer."]);
    rig.start(cx, &waiting(json!({"verdict": {"type": "string"}})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(node(&run, "dev").status, NodeStatus::WaitingForUser);
    assert_eq!(
        rig.fake.world().waits,
        [WaitNotice {
            session_id: "s1".into(),
            workflow: "Ship it".into(),
            step: "dev".into()
        }]
    );
    // Approvals stay in the session: the step keeps waiting for the user, not for them.
    rig.fake.set_waiting("s1", true);
    minutes(cx, 30);
    assert_eq!(
        node(&rig.last_run(), "dev").status,
        NodeStatus::WaitingForUser
    );
    // The user chats on, and the session records the value.
    record(&rig, "s1", json!({"verdict": "ship it"})).unwrap();
    assert!(rig.fake.world().wait_overs.is_empty());
    answer(&rig, "s1", WaitChoice::Continue).unwrap();
    // Another press of either button, before the step has even moved on, finds nothing to answer.
    assert!(answer(&rig, "s1", WaitChoice::Continue).is_err());
    assert!(answer(&rig, "s1", WaitChoice::Fail).is_err());
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(output_of(&run, "dev"), "verdict=\"ship it\"");
    assert_eq!(text(&run, "dev", "reply"), "First answer.");
    let w = rig.fake.world();
    assert_eq!(w.wait_overs, ["s1"]);
    assert!(w.stops.is_empty() && w.extract_calls.is_empty());
    drop(w);
    // The step is over: nothing more is accepted for it.
    assert!(answer(&rig, "s1", WaitChoice::Continue).is_err());
    assert!(record(&rig, "s1", json!({"verdict": "late"})).is_err());
}

#[gpui::test]
fn continue_reads_the_whole_conversation_for_what_was_not_recorded(cx: &mut TestAppContext) {
    let rig = Rig::new(&["First answer."]);
    rig.fake
        .extraction(Ok(fenced(json!({"verdict": "from the chat"}))));
    rig.start(cx, &waiting(json!({"verdict": {"type": "string"}})));
    cx.run_until_parked();
    // The first turn ending is not the end: nothing is read until the user continues.
    assert!(rig.fake.world().extract_calls.is_empty());
    answer(&rig, "s1", WaitChoice::Continue).unwrap();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(output_of(&run, "dev"), "verdict=\"from the chat\"");
    assert_eq!(rig.fake.world().extract_calls[0].session_id, "s1");
}

#[gpui::test]
fn fail_step_fails_the_node_and_leaves_the_session_open(cx: &mut TestAppContext) {
    let rig = Rig::new(&["First answer."]);
    rig.start(cx, &waiting(json!({})));
    cx.run_until_parked();
    answer(&rig, "s1", WaitChoice::Fail).unwrap();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let dev = node(&run, "dev");
    assert_eq!(dev.status, NodeStatus::Failed);
    assert!(dev.error.as_deref().unwrap().contains("Fail step"));
    let w = rig.fake.world();
    assert!(w.stops.is_empty(), "the session is not stopped");
    assert_eq!(w.wait_overs, ["s1"]);
}

#[gpui::test]
fn nobody_answering_fails_the_step_after_the_wait_limit(cx: &mut TestAppContext) {
    let rig = Rig::with_limits(
        &["First answer."],
        Limits {
            approval_wait_ms: 10_000,
            ..Limits::default()
        },
    );
    rig.start(cx, &waiting(json!({})));
    cx.run_until_parked();
    assert_eq!(
        node(&rig.last_run(), "dev").status,
        NodeStatus::WaitingForUser
    );
    minutes(cx, 12);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let error = node(&run, "dev").error.clone().unwrap();
    assert!(error.contains("wait limit"), "{error}");
    assert_eq!(rig.fake.world().wait_overs, ["s1"]);
    assert!(answer(&rig, "s1", WaitChoice::Continue).is_err());
}

#[gpui::test]
fn stop_withdraws_the_banner_and_a_later_continue_is_refused(cx: &mut TestAppContext) {
    let rig = Rig::new(&["First answer."]);
    rig.start(cx, &waiting(json!({})));
    cx.run_until_parked();
    assert!(rig.runner.stop(WF, None));
    // Stop and Continue race: whichever the run hears second changes nothing, so the run is
    // stopped and the step never finishes, whether or not this answer got in.
    let _ = answer(&rig, "s1", WaitChoice::Continue);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(node(&run, "dev").status, NodeStatus::Stopped);
    assert_eq!(rig.fake.world().wait_overs, ["s1"]);
    assert!(answer(&rig, "s1", WaitChoice::Continue).is_err());
}

#[gpui::test]
fn a_wait_for_me_step_of_an_auto_node_next_to_it_is_not_held(cx: &mut TestAppContext) {
    let def = workflow(
        vec![
            trigger(),
            session("auto", json!({}), json!({})),
            session("wait", json!({}), json!({"mode": "waitForMe"})),
        ],
        vec![edge("start", "auto"), edge("start", "wait")],
    );
    let rig = Rig::new(&["a", "b"]);
    rig.start(cx, &def);
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(node(&run, "auto").status, NodeStatus::Done);
    assert_eq!(node(&run, "wait").status, NodeStatus::WaitingForUser);
    assert_eq!(rig.fake.world().waits.len(), 1);
}

#[gpui::test]
fn continue_waits_for_a_running_turn_before_taking_outputs(cx: &mut TestAppContext) {
    let rig = Rig::new(&["First answer."]);
    rig.fake
        .extraction(Ok(fenced(json!({"verdict": "from the chat"}))));
    rig.start(cx, &waiting(json!({"verdict": {"type": "string"}})));
    cx.run_until_parked();
    // The user chatted on and the session is still working when they press Continue.
    let busy = rig.fake.busy();
    answer(&rig, "s1", WaitChoice::Continue).unwrap();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(node(&run, "dev").status, NodeStatus::WaitingForUser);
    {
        let w = rig.fake.world();
        assert_eq!(w.turn_end_calls, ["s1"]);
        assert!(w.extract_calls.is_empty(), "nothing is read mid-turn");
        assert!(w.wait_overs.is_empty(), "the banner stays");
    }
    // The turn ends: the outputs are taken now.
    busy.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(output_of(&run, "dev"), "verdict=\"from the chat\"");
    assert_eq!(rig.fake.world().wait_overs, ["s1"]);
}

#[gpui::test]
fn stop_while_continue_waits_for_the_turn_takes_no_outputs(cx: &mut TestAppContext) {
    let rig = Rig::new(&["First answer."]);
    rig.start(cx, &waiting(json!({"verdict": {"type": "string"}})));
    cx.run_until_parked();
    let _busy = rig.fake.busy();
    answer(&rig, "s1", WaitChoice::Continue).unwrap();
    cx.run_until_parked();
    assert!(rig.runner.stop(WF, None));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Stopped);
    assert_eq!(node(&run, "dev").status, NodeStatus::Stopped);
    let w = rig.fake.world();
    assert!(w.extract_calls.is_empty());
    assert_eq!(w.wait_overs, ["s1"]);
}

// --- a value recorded while the outputs are read ---------------------------------------------

#[gpui::test]
fn a_value_recorded_while_the_fields_are_read_is_kept_and_wins(cx: &mut TestAppContext) {
    let rig = Rig::new(&["First answer."]);
    rig.fake
        .extraction(Ok(fenced(json!({"verdict": "from the chat"}))));
    let gate = rig.fake.extraction_gate();
    rig.start(cx, &waiting(json!({"verdict": {"type": "string"}})));
    cx.run_until_parked();
    answer(&rig, "s1", WaitChoice::Continue).unwrap();
    cx.run_until_parked();
    // The read is under way: the step works again, not waiting on the user.
    assert_eq!(rig.fake.world().extract_calls.len(), 1);
    assert_eq!(node(&rig.last_run(), "dev").status, NodeStatus::Running);
    // The session records the value now: accepted, so it must not be lost.
    let recorded = record(&rig, "s1", json!({"verdict": "recorded"})).unwrap();
    assert_eq!(recorded.recorded, ["verdict"]);
    gate.close();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(output_of(&run, "dev"), "verdict=\"recorded\"");
    // Once the step is over, nothing more is accepted.
    let late = record(&rig, "s1", json!({"verdict": "late"})).unwrap_err();
    assert!(late.contains("not running a workflow step"), "{late}");
}

// --- Wait for me after a turn that did not complete -------------------------------------------

#[gpui::test]
fn a_first_turn_the_user_stopped_still_waits_for_them(cx: &mut TestAppContext) {
    let rig = Rig::new(&["Half an answer."]);
    rig.fake.end_turns_with(Outcome::cancelled(STOPPED_BY_USER));
    rig.start(cx, &waiting(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(node(&run, "dev").status, NodeStatus::WaitingForUser);
    assert_eq!(rig.fake.world().waits.len(), 1);
    answer(&rig, "s1", WaitChoice::Continue).unwrap();
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Done);
}

#[gpui::test]
fn a_failed_first_turn_waits_for_the_user_while_its_session_exists(cx: &mut TestAppContext) {
    let rig = Rig::new(&["Oops."]);
    rig.fake
        .end_turns_with(Outcome::failed("The provider errored"));
    rig.start(cx, &waiting(json!({})));
    cx.run_until_parked();
    assert_eq!(
        node(&rig.last_run(), "dev").status,
        NodeStatus::WaitingForUser
    );
    answer(&rig, "s1", WaitChoice::Fail).unwrap();
    cx.run_until_parked();
    assert_eq!(rig.last_run().status, RunStatus::Failed);
}

#[gpui::test]
fn a_failed_turn_of_an_auto_step_still_fails_it(cx: &mut TestAppContext) {
    let rig = Rig::new(&["Oops."]);
    rig.fake
        .end_turns_with(Outcome::failed("The provider errored"));
    rig.start(cx, &simple(json!({})));
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert!(rig.fake.world().waits.is_empty());
}

// --- the runner's own limits still fail a Wait for me step -----------------------------------

fn waiting_with(extra: Value) -> Workflow {
    workflow(
        vec![trigger(), session("dev", json!({}), extra)],
        vec![edge("start", "dev")],
    )
}

#[gpui::test]
fn the_step_timeout_fails_a_wait_for_me_step_with_its_reason(cx: &mut TestAppContext) {
    let rig = Rig::new(&["late"]);
    rig.fake.hold();
    rig.start(
        cx,
        &waiting_with(json!({"mode": "waitForMe", "timeoutMinutes": 1})),
    );
    cx.run_until_parked();
    minutes(cx, 62);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    let error = node(&run, "dev").error.clone().unwrap();
    assert!(error.contains("took longer than 1 minutes"), "{error}");
    assert!(rig.fake.world().waits.is_empty(), "the user was not asked");
}

#[gpui::test]
fn an_unanswered_approval_fails_a_wait_for_me_step_without_a_second_notice(
    cx: &mut TestAppContext,
) {
    let rig = Rig::with_limits(
        &[],
        Limits {
            approval_wait_ms: 10_000,
            ..Limits::default()
        },
    );
    rig.fake.hold();
    rig.start(cx, &waiting_with(json!({"mode": "waitForMe"})));
    cx.run_until_parked();
    rig.fake.set_waiting("s1", true);
    minutes(cx, 12);
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(
        node(&run, "dev").error.as_deref(),
        Some("Nobody answered its approval request.")
    );
    assert!(rig.fake.world().waits.is_empty(), "no needs-you notice");
}

#[gpui::test]
fn a_provider_failure_handed_over_stays_visible_until_the_step_finishes(cx: &mut TestAppContext) {
    let rig = Rig::new(&["Oops."]);
    rig.fake
        .end_turns_with(Outcome::failed("The provider errored"));
    rig.start(cx, &waiting(json!({})));
    cx.run_until_parked();
    let dev = node(&rig.last_run(), "dev").clone();
    assert_eq!(dev.status, NodeStatus::WaitingForUser);
    assert!(
        dev.error
            .as_deref()
            .unwrap()
            .contains("The provider errored"),
        "{:?}",
        dev.error
    );
    answer(&rig, "s1", WaitChoice::Continue).unwrap();
    cx.run_until_parked();
    let run = rig.last_run();
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(node(&run, "dev").error, None);
}
