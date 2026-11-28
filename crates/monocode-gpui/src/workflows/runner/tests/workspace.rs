//! Concurrent runs, the run worktree and `onFailure: continue`, against the fake host.
use super::*;

/// A workflow with a per run workspace on `{{inputs.repo}}`; steps that name no project or
/// checkout work in the run worktree.
fn spaced(nodes: Vec<Value>, edges: Vec<Value>, extra: Value) -> Workflow {
    let mut def = json!({
        "id": WF, "name": "Ship it",
        "inputs": {"repo": {"type": "project"}, "lib": {"type": "project"}},
        "inputValues": {"repo": "/code/app", "lib": "/code/lib"},
        "workspace": {"mode": "perRun", "project": "{{inputs.repo}}"},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 0,
    });
    for (key, value) in extra.as_object().unwrap() {
        def[key] = value.clone();
    }
    let built = parse_workflow(&Json::from(&def)).expect("a workflow");
    assert_eq!(validate_workflow(&built), vec![]);
    built
}

fn plain_session(id: &str, extra: Value) -> Value {
    let mut node = json!({"id": id, "type": "session", "prompt": format!("Do {id}")});
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

fn plain_command(id: &str, extra: Value) -> Value {
    let mut node = json!({"id": id, "type": "command", "command": "make test"});
    for (key, value) in extra.as_object().unwrap() {
        node[key] = value.clone();
    }
    node
}

fn numbered(def: &Workflow, id: &str, number: u64) -> WorkflowRun {
    let mut run = start_run(def, RunCaller::new(CallerKind::Manual, "start"), 1_000, id);
    run.number = Some(number);
    run
}

fn saved(rig: &Rig, id: &str) -> WorkflowRun {
    rig.fake
        .world()
        .saves
        .iter()
        .rev()
        .find(|run| run.id == id)
        .cloned()
        .expect("the run was saved")
}

// --- concurrent runs ---------------------------------------------------------------------------

#[gpui::test]
fn two_runs_of_one_workflow_go_at_the_same_time(cx: &mut TestAppContext) {
    let def = spaced(
        vec![trigger(), plain_session("dev", json!({"checkout": "root"}))],
        vec![edge("start", "dev")],
        json!({"concurrency": {"max": 2}}),
    );
    let rig = Rig::new(&["one", "two"]);
    let hold = rig.fake.hold();
    assert!(rig.start_run(cx, numbered(&def, "run-1", 1)));
    assert!(rig.start_run(cx, numbered(&def, "run-2", 2)));
    // The same run twice, or a third run over the two slots, is refused.
    assert!(!rig.start_run(cx, numbered(&def, "run-2", 2)));
    assert!(!rig.start_run(cx, numbered(&def, "run-3", 3)));
    cx.run_until_parked();

    assert!(rig.runner.is_running(WF));
    assert_eq!(rig.runner.live_count(WF), 2);
    assert_eq!(rig.signals.live_runs(WF), 2);
    let mut going = rig.runner.running();
    going.sort();
    assert_eq!(
        going,
        [
            (WF.to_string(), "run-1".into()),
            (WF.to_string(), "run-2".into())
        ]
    );
    // Each has its own session.
    let sessions = |id: &str| node(&saved(&rig, id), "dev").session_id.clone();
    assert_ne!(sessions("run-1"), sessions("run-2"));
    assert!(sessions("run-1").is_some() && sessions("run-2").is_some());

    hold.close();
    cx.run_until_parked();
    assert_eq!(saved(&rig, "run-1").status, RunStatus::Done);
    assert_eq!(saved(&rig, "run-2").status, RunStatus::Done);
    assert!(!rig.runner.is_running(WF));
    assert_eq!(rig.signals.live_runs(WF), 0);
    // Each run posted its own summary.
    assert_eq!(rig.fake.world().posts.len(), 2);
}

#[gpui::test]
fn stopping_one_run_leaves_the_others_going(cx: &mut TestAppContext) {
    let def = spaced(
        vec![trigger(), plain_session("dev", json!({"checkout": "root"}))],
        vec![edge("start", "dev")],
        json!({}),
    );
    let rig = Rig::new(&["one", "two"]);
    let hold = rig.fake.hold();
    rig.start_run(cx, numbered(&def, "run-1", 1));
    rig.start_run(cx, numbered(&def, "run-2", 2));
    cx.run_until_parked();
    let first = node(&saved(&rig, "run-1"), "dev")
        .session_id
        .clone()
        .unwrap();

    // Another workflow's id, or a run that is not there, stops nothing.
    assert!(!rig.runner.stop("wf-other000", Some("run-1")));
    assert!(!rig.runner.stop(WF, Some("nobody")));
    assert!(rig.runner.stop(WF, Some("run-1")));
    assert!(
        !rig.runner.stop(WF, Some("run-1")),
        "a second stop changes nothing"
    );
    cx.run_until_parked();

    assert_eq!(saved(&rig, "run-1").status, RunStatus::Stopped);
    assert_eq!(saved(&rig, "run-2").status, RunStatus::Running);
    assert_eq!(rig.fake.world().stops, [first]);
    assert!(rig.runner.is_running(WF));
    assert_eq!(rig.runner.live_count(WF), 1);
    assert_eq!(rig.signals.live_runs(WF), 1);
    // The stopped run posted its report; the other has not.
    assert_eq!(rig.fake.world().posts.len(), 1);
    assert_eq!(rig.fake.world().posts[0].run.id, "run-1");

    hold.close();
    cx.run_until_parked();
    assert_eq!(saved(&rig, "run-2").status, RunStatus::Done);
    assert_eq!(saved(&rig, "run-1").status, RunStatus::Stopped);
}

#[gpui::test]
fn stopping_a_workflow_without_a_run_id_stops_every_run_it_has(cx: &mut TestAppContext) {
    let def = spaced(
        vec![trigger(), plain_session("dev", json!({"checkout": "root"}))],
        vec![edge("start", "dev")],
        json!({}),
    );
    let rig = Rig::new(&["one", "two"]);
    let _hold = rig.fake.hold();
    rig.start_run(cx, numbered(&def, "run-1", 1));
    rig.start_run(cx, numbered(&def, "run-2", 2));
    cx.run_until_parked();
    assert!(rig.runner.stop(WF, None));
    cx.run_until_parked();
    assert_eq!(saved(&rig, "run-1").status, RunStatus::Stopped);
    assert_eq!(saved(&rig, "run-2").status, RunStatus::Stopped);
    assert!(!rig.runner.is_running(WF));
}

#[gpui::test]
fn a_finished_run_asks_for_a_look_so_the_queue_can_start_its_next_run(cx: &mut TestAppContext) {
    let def = spaced(
        vec![trigger(), plain_session("dev", json!({"checkout": "root"}))],
        vec![edge("start", "dev")],
        json!({}),
    );
    let rig = Rig::new(&["done"]);
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    assert_eq!(saved(&rig, "run-1").status, RunStatus::Done);
    assert!(rig.signals.take_check_now(), "the free slot was announced");
}

// --- the run worktree --------------------------------------------------------------------------

#[gpui::test]
fn the_run_makes_one_worktree_and_sessions_and_commands_work_in_it(cx: &mut TestAppContext) {
    let def = spaced(
        vec![
            trigger(),
            plain_session("dev", json!({"prompt": "Work in {{run.worktree}}"})),
            plain_command("test", json!({})),
        ],
        vec![edge("start", "dev"), edge("dev", "test")],
        json!({}),
    );
    let rig = Rig::new(&["done"]);
    rig.start_run(cx, numbered(&def, "run-1", 7));
    cx.run_until_parked();

    let tree = "/code/app-worktrees/wf-ship-it-7";
    let w = rig.fake.world();
    assert_eq!(w.run_trees.len(), 1);
    assert_eq!(
        w.run_trees[0],
        RunWorktreeRequest {
            project: "/code/app".into(),
            base_branch: "main".into(),
            slug: "ship-it".into(),
            number: 7,
            run_id: "run-1".into(),
        }
    );
    // The session started in the project, working in the run worktree.
    assert_eq!(w.starts[0].project, "/code/app");
    assert_eq!(w.starts[0].worktree_cwd.as_deref(), Some(tree));
    assert!(!w.starts[0].new_worktree);
    // And the command runs there too.
    assert_eq!(w.commands[0].0, tree);
    // Changes are measured from where the run started, not from this step's start.
    assert_eq!(w.fact_calls[0].1.as_deref(), Some("run-base"));
    assert!(w.head_calls.is_empty());
    drop(w);
    let run = saved(&rig, "run-1");
    assert_eq!(run.status, RunStatus::Done);
    let made = run.worktree.expect("the run keeps its worktree");
    assert_eq!(
        (made.path.as_str(), made.branch.as_str(), made.base.as_str()),
        (tree, "wf/ship-it-7", "run-base")
    );
    // The scope says where the run works.
    assert!(
        rig.fake.world().starts[0]
            .prompt
            .contains(&format!("Work in {tree}"))
    );
    // The worktree is kept after the run: the host was never asked to remove anything.
    assert!(rig.fake.world().stops.is_empty());
}

#[gpui::test]
fn a_step_with_its_own_checkout_or_cwd_keeps_out_of_the_run_worktree(cx: &mut TestAppContext) {
    let def = spaced(
        vec![
            trigger(),
            plain_session("dev", json!({"checkout": "root"})),
            plain_command("test", json!({"cwd": "root"})),
        ],
        vec![edge("start", "dev"), edge("dev", "test")],
        json!({}),
    );
    let rig = Rig::new(&["done"]);
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.starts[0].worktree_cwd, None);
    assert_eq!(w.commands[0].0, "/code/app");
    // The worktree is still made for the run: other steps may use it.
    assert_eq!(w.run_trees.len(), 1);
}

#[gpui::test]
fn a_workspace_with_mode_none_makes_no_worktree(cx: &mut TestAppContext) {
    let def = spaced(
        vec![
            trigger(),
            plain_session("dev", json!({})),
            plain_command("test", json!({})),
        ],
        vec![edge("start", "dev"), edge("dev", "test")],
        json!({"workspace": {"mode": "none", "project": "{{inputs.repo}}"}, "concurrency": {"max": 1}}),
    );
    let rig = Rig::new(&["done"]);
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert!(w.run_trees.is_empty());
    assert_eq!(w.starts[0].worktree_cwd, None);
    assert_eq!(w.commands[0].0, "/code/app");
    drop(w);
    assert!(saved(&rig, "run-1").worktree.is_none());
}

#[gpui::test]
fn a_run_that_cannot_make_its_worktree_fails_before_any_step(cx: &mut TestAppContext) {
    let def = spaced(
        vec![trigger(), plain_session("dev", json!({}))],
        vec![edge("start", "dev")],
        json!({}),
    );
    let rig = Rig::new(&["done"]);
    rig.fake
        .trees_fail("The base branch \"main\" does not exist");
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    let run = saved(&rig, "run-1");
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error
            .as_deref()
            .unwrap()
            .contains("Could not make the run's worktree")
    );
    assert!(run.error.as_deref().unwrap().contains("does not exist"));
    assert!(rig.fake.world().starts.is_empty());
    assert!(!rig.runner.is_running(WF));
    // The failure is reported like any other.
    assert_eq!(rig.fake.world().posts.len(), 1);
}

#[gpui::test]
fn a_resumed_run_goes_on_in_the_worktree_it_has(cx: &mut TestAppContext) {
    let def = spaced(
        vec![trigger(), plain_session("dev", json!({}))],
        vec![edge("start", "dev")],
        json!({}),
    );
    let mut run = numbered(&def, "run-1", 3);
    run.worktree = Some(RunWorktree {
        path: "/code/app-worktrees/earlier".into(),
        branch: "wf/ship-it-3".into(),
        base: "old-base".into(),
        removed_at: None,
    });
    let rig = Rig::new(&["done"]);
    rig.start_run(cx, run);
    cx.run_until_parked();
    let w = rig.fake.world();
    assert!(w.run_trees.is_empty(), "no second worktree");
    assert_eq!(
        w.starts[0].worktree_cwd.as_deref(),
        Some("/code/app-worktrees/earlier")
    );
}

#[gpui::test]
fn a_step_in_another_project_is_refused_the_run_worktree_at_run_time_too(cx: &mut TestAppContext) {
    // Validation refuses this, but a definition from an older file may slip past: the run says so
    // instead of working in the wrong project's folder.
    let mut def = spaced(
        vec![
            trigger(),
            plain_session(
                "dev",
                json!({"project": "{{inputs.lib}}", "checkout": "root"}),
            ),
        ],
        vec![edge("start", "dev")],
        json!({}),
    );
    if let NodeKind::Session { checkout, .. } = &mut def.nodes[1].kind {
        *checkout = Checkout::Run;
    }
    let rig = Rig::new(&["done"]);
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    let run = saved(&rig, "run-1");
    assert_eq!(node(&run, "dev").status, NodeStatus::Failed);
    assert!(
        node(&run, "dev")
            .error
            .as_deref()
            .unwrap()
            .contains("another project")
    );
    assert!(rig.fake.world().starts.is_empty());
}

#[gpui::test]
fn an_existing_folder_is_where_the_step_works(cx: &mut TestAppContext) {
    let def = spaced(
        vec![
            trigger(),
            plain_session(
                "dev",
                json!({"checkout": {"existing": "/code/app-worktrees/mine"}}),
            ),
        ],
        vec![edge("start", "dev")],
        json!({}),
    );
    let rig = Rig::new(&["done"]);
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    assert_eq!(
        rig.fake.world().starts[0].worktree_cwd.as_deref(),
        Some("/code/app-worktrees/mine")
    );
}

// --- For each branches ---------------------------------------------------------------------------

#[gpui::test]
fn each_branch_of_a_for_each_gets_a_worktree_forked_from_the_run_worktree(cx: &mut TestAppContext) {
    let def = spaced(
        vec![
            trigger(),
            plain_session("fetch", json!({"outputs": {"tickets": {"type": "list"}}})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"}),
            plain_command("cmd", json!({"parent": "each", "command": "echo {{item}}"})),
        ],
        vec![edge("start", "fetch"), edge("fetch", "each")],
        json!({}),
    );
    let rig = Rig::new(&[&fenced(json!({"tickets": ["a", "b"]}))]);
    rig.start_run(cx, numbered(&def, "run-1", 2));
    cx.run_until_parked();
    let w = rig.fake.world();
    assert_eq!(w.run_trees.len(), 1);
    let mut forks = w.branch_trees.clone();
    forks.sort();
    assert_eq!(forks, [("each".to_string(), 0), ("each".to_string(), 1)]);
    // The first step runs in the run worktree; the branches' commands in their own.
    let run_tree = "/code/app-worktrees/wf-ship-it-2";
    assert_eq!(w.starts[0].worktree_cwd.as_deref(), Some(run_tree));
    let mut dirs: Vec<&str> = w.commands.iter().map(|c| c.0.as_str()).collect();
    dirs.sort();
    assert_eq!(
        dirs,
        [format!("{run_tree}-each-0"), format!("{run_tree}-each-1")]
    );
    drop(w);
    let run = saved(&rig, "run-1");
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(
        run.branch_worktrees.keys().cloned().collect::<Vec<_>>(),
        ["each#0", "each#1"]
    );
    assert_eq!(run.branch_worktrees["each#1"].branch, "wf/ship-it-2-each-1");
}

#[gpui::test]
fn a_branchs_worktree_is_reused_for_every_round_and_step_of_that_index(cx: &mut TestAppContext) {
    let def = spaced(
        vec![
            trigger(),
            plain_session("fetch", json!({"outputs": {"tickets": {"type": "list"}}})),
            json!({"id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item"}),
            plain_session(
                "dev",
                json!({"parent": "each", "reuseSession": false, "outputs": {"summary": {"type": "string"}}}),
            ),
            plain_session(
                "review",
                json!({"parent": "each", "outputs": {"notes": {"type": "string"}}}),
            ),
        ],
        vec![
            edge("start", "fetch"),
            edge("fetch", "each"),
            edge("dev", "review"),
            json!({"from": "review", "to": "dev", "loop": {"maxRounds": 2, "decide": "ai"}}),
        ],
        json!({}),
    );
    let rig = Rig::new(&[
        &fenced(json!({"tickets": ["only"]})),
        &fenced(json!({"summary": "a"})),
        &fenced(json!({"notes": "meh"})),
        &fenced(json!({"summary": "b"})),
        &fenced(json!({"notes": "good"})),
    ]);
    rig.fake
        .answer(Ok(fenced(json!({"answer": "loop", "reason": "again"}))));
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    let run = saved(&rig, "run-1");
    assert_eq!(run.status, RunStatus::Done, "{:?}", run.error);
    let w = rig.fake.world();
    // One fork for index 0, however many rounds and steps used it.
    assert_eq!(w.branch_trees, [("each".to_string(), 0)]);
    let in_branch: Vec<_> = w
        .starts
        .iter()
        .filter(|s| s.prompt.starts_with("Do dev") || s.prompt.starts_with("Do review"))
        .map(|s| s.worktree_cwd.clone())
        .collect();
    assert!(in_branch.len() >= 3, "{in_branch:?}");
    assert!(
        in_branch
            .iter()
            .all(|dir| dir.as_deref() == Some("/code/app-worktrees/wf-ship-it-1-each-0"))
    );
}

// --- onFailure ---------------------------------------------------------------------------------

#[gpui::test]
fn a_failed_command_finishes_the_node_with_its_output_when_the_workflow_carries_on(
    cx: &mut TestAppContext,
) {
    let def = spaced(
        vec![
            trigger(),
            plain_command("test", json!({"onFailure": "continue"})),
        ],
        vec![edge("start", "test")],
        json!({}),
    );
    let rig = Rig::new(&[]);
    rig.fake
        .command_returns(command_result(Some(2), "3 failed", "see above"));
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    let run = saved(&rig, "run-1");
    assert_eq!(run.status, RunStatus::Done);
    let test = node(&run, "test");
    assert_eq!(test.status, NodeStatus::Done);
    let output = test.output.as_ref().unwrap();
    assert_eq!(output.get("exitCode"), Some(&Json::Number(2.0)));
    assert_eq!(output.get("stdout"), Some(&Json::String("3 failed".into())));
    assert_eq!(
        output.get("stderr"),
        Some(&Json::String("see above".into()))
    );
    assert!(test.error.is_none());
}

#[gpui::test]
fn without_on_failure_continue_a_failed_command_still_fails_the_node(cx: &mut TestAppContext) {
    for extra in [json!({}), json!({"onFailure": "fail"})] {
        let def = spaced(
            vec![trigger(), plain_command("test", extra)],
            vec![edge("start", "test")],
            json!({}),
        );
        let rig = Rig::new(&[]);
        rig.fake
            .command_returns(command_result(Some(2), "x", "boom"));
        rig.start_run(cx, numbered(&def, "run-1", 1));
        cx.run_until_parked();
        let test = node(&saved(&rig, "run-1"), "test").clone();
        assert_eq!(test.status, NodeStatus::Failed);
        assert_eq!(
            test.error.as_deref(),
            Some("The command exited with code 2: boom")
        );
    }
}

#[gpui::test]
fn a_timeout_or_a_cancel_still_fails_the_node_even_with_continue(cx: &mut TestAppContext) {
    for (timed_out, cancelled, said) in [
        (true, false, "The command timed out after 10 minutes"),
        (false, true, "The command was cancelled"),
    ] {
        let def = spaced(
            vec![
                trigger(),
                plain_command("test", json!({"onFailure": "continue"})),
            ],
            vec![edge("start", "test")],
            json!({}),
        );
        let rig = Rig::new(&[]);
        let mut result = command_result(Some(1), "", "");
        result.timed_out = timed_out;
        result.cancelled = cancelled;
        rig.fake.command_returns(result);
        rig.start_run(cx, numbered(&def, "run-1", 1));
        cx.run_until_parked();
        let test = node(&saved(&rig, "run-1"), "test").clone();
        assert_eq!(test.status, NodeStatus::Failed);
        assert_eq!(test.error.as_deref(), Some(said));
    }
}

#[gpui::test]
fn an_if_can_branch_on_a_failed_test_run(cx: &mut TestAppContext) {
    let def = spaced(
        vec![
            trigger(),
            plain_command("test", json!({"onFailure": "continue"})),
            json!({"id": "gate", "type": "if", "condition": "{{test.exitCode}} != 0"}),
            plain_session("fix", json!({})),
        ],
        vec![
            edge("start", "test"),
            edge("test", "gate"),
            json!({"from": "gate", "to": "fix", "handle": "true"}),
        ],
        json!({}),
    );
    let rig = Rig::new(&["fixed"]);
    rig.fake.command_returns(command_result(Some(1), "red", ""));
    rig.start_run(cx, numbered(&def, "run-1", 1));
    cx.run_until_parked();
    let run = saved(&rig, "run-1");
    assert_eq!(node(&run, "fix").status, NodeStatus::Done);
    assert_eq!(rig.fake.world().starts.len(), 1);
}
