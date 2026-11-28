//! The Workflows reads of the web app: who sees which workflow, and which sessions a run links.
use super::workflows::{
    CALLER_KEYS, LastRun, Links, Loaded, NODE_KEYS, RUN_KEYS, bound_runs, read_files, reply_of,
    session_ids_of, workflow_visible,
};
use super::*;
use monocode_core::mobile::{DeviceKind, Permission};
use monocode_core::mono::workflows::global::WorkflowsDir;
use monocode_core::mono::workflows::model::{
    CallerKind, Checkout, Decision, InputType, InputValue, Node, NodeKind, NodeRun, NodeStatus,
    RoundRecord, RunCaller, RunStatus, RunSummary, Workflow, WorkflowInput, WorkflowRun,
};
use monocode_core::mono::workflows::ordered::Json;
use monocode_core::mono::workflows::store::{update_runs, update_workflows};

fn web(projects: Option<Vec<String>>, monos: Option<Vec<String>>) -> DeviceContext {
    DeviceContext {
        id: "web".into(),
        kind: DeviceKind::Web,
        permission: Permission::Read,
        projects,
        monos,
    }
}

fn session_node(id: &str, project: &str) -> Node {
    Node::new(
        id,
        NodeKind::Session {
            project: project.into(),
            prompt: "p".into(),
            checkout: Checkout::Root,
            output: Default::default(),
            mode: None,
            surface: None,
            harness: None,
            model: None,
            effort: None,
            reuse_session: None,
            nest_under: None,
            timeout_minutes: None,
            target: None,
            account: None,
            access: None,
            folder: None,
            link_work_item: None,
        },
    )
}

/// A workflow whose project inputs hold `paths`, each used by one session step.
fn workflow(id: &str, paths: &[&str]) -> Workflow {
    let mut w = Workflow::new(id, id);
    for (i, path) in paths.iter().enumerate() {
        let name = format!("p{i}");
        w.inputs
            .insert(name.clone(), WorkflowInput::new(InputType::Project));
        w.input_values
            .insert(name.clone(), InputValue::String((*path).into()));
        w.nodes.push(session_node(
            &format!("s{i}"),
            &format!("{{{{inputs.{name}}}}}"),
        ));
    }
    w
}

fn run_of(w: &Workflow, sessions: &[(&str, &str)]) -> WorkflowRun {
    let mut nodes = Vec::new();
    for (node, session) in sessions {
        let mut run = NodeRun::new(*node, "");
        run.status = NodeStatus::Done;
        run.session_id = Some((*session).into());
        run.output = None;
        nodes.push(run);
    }
    WorkflowRun {
        id: "run-1".into(),
        workflow_id: w.id.clone(),
        definition: w.clone(),
        status: RunStatus::Done,
        started_at: 1_000,
        ended_at: Some(4_000),
        caller: RunCaller::new(CallerKind::Manual, "manual"),
        items: Default::default(),
        nodes,
        report_posted_at: None,
        error: None,
        cloud_instances: Vec::new(),
        summary: Some(RunSummary {
            title: "Done".into(),
            text: "All good".into(),
        }),
        number: None,
        queued_at: None,
        worktree: None,
        branch_worktrees: Default::default(),
    }
}

#[test]
fn a_workflow_is_visible_only_when_every_project_it_uses_is() {
    let known = vec!["/work/a".to_owned(), "/work/b".to_owned()];
    let (a, b) = (project_id("/work/a"), project_id("/work/b"));
    let both = workflow("both", &["/work/a", "/work/b"]);
    let only_a = workflow("a", &["/work/a"]);
    let outside = workflow("x", &["/work/zzz"]);
    let all = web(None, None);
    let just_a = web(Some(vec![a.clone()]), None);
    let both_sel = web(Some(vec![a, b]), None);
    let none = web(Some(vec![]), None);

    // No selection: everything, even a workflow over an unknown project.
    assert!(workflow_visible(&all, &known, &outside));
    assert!(workflow_visible(&just_a, &known, &only_a));
    assert!(
        !workflow_visible(&just_a, &known, &both),
        "only some of its projects"
    );
    assert!(workflow_visible(&both_sel, &known, &both));
    assert!(
        !workflow_visible(&both_sel, &known, &outside),
        "unknown project"
    );
    assert!(!workflow_visible(&none, &known, &only_a));
}

#[test]
fn a_project_that_cannot_be_worked_out_hides_the_workflow_from_a_narrowed_device() {
    let known = vec!["/work/a".to_owned()];
    let narrowed = web(Some(vec![project_id("/work/a")]), None);
    // A step takes its project from a previous step.
    let mut dynamic = workflow("d", &["/work/a"]);
    dynamic.nodes.push(session_node("s9", "{{s0.path}}"));
    assert!(!workflow_visible(&narrowed, &known, &dynamic));
    // A step names an input that has no value.
    let mut empty = workflow("e", &["/work/a"]);
    empty.input_values.remove("p0");
    assert!(!workflow_visible(&narrowed, &known, &empty));
    // Not declared at all.
    let mut undeclared = Workflow::new("u", "u");
    undeclared.nodes.push(session_node("s0", "{{inputs.nope}}"));
    assert!(!workflow_visible(&narrowed, &known, &undeclared));
    // The same workflows are fine for a device with no selection.
    for w in [&dynamic, &empty, &undeclared] {
        assert!(workflow_visible(&web(None, None), &known, w));
    }
}

fn project_of(shell: &Shell, session: i64, cx: &App) -> String {
    let key = monocode_core::files::path_key(&shell.session_summaries[&session].cwd);
    shell
        .mobile_projects(cx)
        .into_iter()
        .find(|p| monocode_core::files::path_key(p) == key)
        .expect("the session lives in a rail project")
}

/// What `mobile_workflows` does after reading: resolve the sessions on the UI thread, then shape.
fn reply(shell: &mut Shell, cx: &App, device: &DeviceContext, loaded: Loaded) -> Value {
    let links = shell.workflow_links(device, session_ids_of(&loaded), cx);
    reply_of(device, loaded, &links)
}

fn runs_of(w: &Workflow, runs: Vec<WorkflowRun>) -> Loaded {
    Loaded::Runs(Some((w.clone(), runs)))
}

fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
    keys.sort();
    keys
}

#[gpui::test]
fn runs_link_only_the_sessions_a_device_may_see(cx: &mut gpui::TestAppContext) {
    let (window, a, b) = super::super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let (path_a, path_b) = (project_of(shell, a, cx), project_of(shell, b, cx));
            assert_ne!(path_a, path_b);
            let (wire_a, wire_b) = (shell.mobile_session_id(a), shell.mobile_session_id(b));
            let w = workflow("w", &[&path_a]);
            let run = run_of(
                &w,
                &[
                    ("s0", &wire_a),
                    ("s0", &wire_b),
                    ("gone", "no-such-session"),
                ],
            );
            let sessions_of = |reply: &Value| -> Vec<String> {
                reply["runs"][0]["sessions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect()
            };

            // Both projects: the sessions that exist are linked.
            let both = web(Some(vec![project_id(&path_a), project_id(&path_b)]), None);
            let reply_both = reply(shell, cx, &both, runs_of(&w, vec![run.clone()]));
            assert_eq!(
                sessions_of(&reply_both),
                vec![wire_a.clone(), wire_b.clone()]
            );
            assert_eq!(reply_both["runs"][0]["durationMs"], 3_000);
            assert_eq!(reply_both["runs"][0]["summary"]["text"], "All good");
            assert_eq!(
                reply_both["runs"][0]["nodes"][0]["sessions"],
                json!([wire_a.clone()])
            );

            // Only the workflow's project: b's session is not linked.
            let just_a = web(Some(vec![project_id(&path_a)]), None);
            let reply_a = reply(shell, cx, &just_a, runs_of(&w, vec![run.clone()]));
            assert_eq!(sessions_of(&reply_a), vec![wire_a.clone()]);

            // A device with no selection sees them all.
            let reply_all = reply(shell, cx, &web(None, None), runs_of(&w, vec![run]));
            assert_eq!(sessions_of(&reply_all), vec![wire_a, wire_b]);
        })
        .unwrap();
}

#[gpui::test]
fn the_list_shows_what_a_device_may_see_and_its_last_run(cx: &mut gpui::TestAppContext) {
    let (window, a, _b) = super::super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let path_a = project_of(shell, a, cx);
            let mut mine = workflow("mine", &[&path_a]);
            mine.enabled = true;
            mine.next_run_at = Some(9_000);
            let reply = reply(
                shell,
                cx,
                &web(None, None),
                Loaded::List(vec![
                    (mine.clone(), LastRun::Seen(json!("done"), 5)),
                    (workflow("never", &[&path_a]), LastRun::Never),
                    (workflow("bad", &[&path_a]), LastRun::Unreadable),
                ]),
            );
            let entry = &reply["workflows"][0];
            assert_eq!(entry["enabled"], true);
            assert_eq!(entry["nextRunAt"], 9_000);
            assert_eq!(entry["lastRun"], json!({"status": "done", "startedAt": 5}));
            assert_eq!(entry["triggers"], json!([]));
            assert!(entry.get("lastRunUnreadable").is_none());
            assert_eq!(reply["workflows"][1]["lastRun"], Value::Null);
            assert!(reply["workflows"][1].get("lastRunUnreadable").is_none());
            // A history that cannot be read is not "never run".
            assert_eq!(reply["workflows"][2]["lastRunUnreadable"], true);
            assert_eq!(reply["workflows"][2]["lastRun"], Value::Null);
        })
        .unwrap();
}

#[gpui::test]
fn monos_own_sessions_stay_hidden_but_run_sessions_are_ordinary(cx: &mut gpui::TestAppContext) {
    let (window, a, b) = super::super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let (path_a, path_b) = (project_of(shell, a, cx), project_of(shell, b, cx));
            let (wire_a, wire_b) = (shell.mobile_session_id(a), shell.mobile_session_id(b));
            // Session b is a Mono's own chat.
            let monos = shell.monos.clone().unwrap();
            let owner = monos.update(cx, |monos, _| {
                let chat = monos.roster.create(Vec::new()).id.clone();
                monos.roster.get_mut(&chat).unwrap().session_id = Some(b);
                chat
            });
            let w = workflow("w", &[&path_a, &path_b]);
            let run = run_of(&w, &[("s0", &wire_a), ("s1", &wire_b)]);
            let projects = vec![project_id(&path_a), project_id(&path_b)];
            let linked = |shell: &mut Shell, device: &DeviceContext, cx: &App| {
                let value = reply(shell, cx, device, runs_of(&w, vec![run.clone()]));
                value["runs"][0]["sessions"].clone()
            };
            // The run's own session is linked although a Mono selection is set.
            let other_mono = web(Some(projects.clone()), Some(vec!["other".into()]));
            assert_eq!(linked(shell, &other_mono, cx), json!([wire_a.clone()]));
            // The owner selected: the Mono's session is served as before.
            let owner_sel = web(Some(projects), Some(vec![owner]));
            assert_eq!(linked(shell, &owner_sel, cx), json!([wire_a, wire_b]));
        })
        .unwrap();
}

#[gpui::test]
fn a_node_output_session_id_is_mapped_like_a_run_session_and_the_worktree_is_dropped(
    cx: &mut gpui::TestAppContext,
) {
    let (window, a, b) = super::super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let path_a = project_of(shell, a, cx);
            let (wire_a, wire_b) = (shell.mobile_session_id(a), shell.mobile_session_id(b));
            let w = workflow("w", &[&path_a]);
            let output = |session: &str| {
                let text = json!({
                    "sessionId": session,
                    "worktree": "/Users/someone/work/tree",
                    "branch": "feat/x",
                    "commit": "abc123",
                    "changedFiles": ["a.rs"],
                    "diffStat": "1 file changed",
                    "reply": "ok",
                    "custom": 3,
                })
                .to_string();
                match Json::parse(&text).unwrap() {
                    Json::Object(map) => map,
                    _ => unreachable!(),
                }
            };
            let mut mine = run_of(&w, &[("s0", &wire_a)]);
            mine.nodes[0].output = Some(output(&wire_a));
            let mut other = run_of(&w, &[("s0", &wire_b)]);
            other.nodes[0].output = Some(output(&wire_b));
            let device = web(Some(vec![project_id(&path_a)]), None);
            let value = reply(shell, cx, &device, runs_of(&w, vec![mine, other]));
            let seen = &value["runs"][0]["nodes"][0]["output"];
            assert_eq!(
                keys(seen),
                sorted(&[
                    "sessionId",
                    "branch",
                    "commit",
                    "changedFiles",
                    "diffStat",
                    "reply",
                    "custom"
                ])
            );
            assert_eq!(seen["sessionId"], json!(wire_a));
            assert_eq!(seen["branch"], "feat/x");
            // A session the device may not see is not named in the output either.
            let hidden = &value["runs"][1]["nodes"][0]["output"];
            assert!(hidden.get("sessionId").is_none());
            assert!(hidden.get("worktree").is_none());
            assert_eq!(hidden["reply"], "ok");
            assert!(!value.to_string().contains("/Users/someone"));
        })
        .unwrap();
}

#[test]
fn only_allow_listed_keys_reach_a_device() {
    let mut w = workflow("w", &["/work/a"]);
    w.nodes[0].title = Some("Build".into());
    let mut run = run_of(&w, &[("s0", "persisted-1")]);
    run.caller = RunCaller::mono("mono-1", "manual");
    run.caller.item = Json::parse(
        r##"{"source":"github","kind":"pr","ref":"#7","title":"t","url":"https://x/y","repo":"a/b","localPath":"/Users/me/x","token":"nope"}"##,
    );
    run.items
        .insert("s0".into(), vec![Json::String("/Users/me/x".into())]);
    run.cloud_instances = Vec::new();
    run.error = Some("boom".into());
    let node = &mut run.nodes[0];
    node.reuse_session_id = Some("reuse".into());
    node.interrupted_sessions = Some(vec!["old".into()]);
    node.reasked = Some(true);
    node.started_at = Some(1_100);
    node.ended_at = Some(1_900);
    node.error = Some("bad".into());
    node.decision = Some(Decision {
        value: "yes".into(),
        reason: Some("because".into()),
    });
    node.pending_output = Json::parse(r#"{"x":"/Users/me/pending"}"#).and_then(|j| match j {
        Json::Object(map) => Some(map),
        _ => None,
    });
    node.history = Some(vec![RoundRecord {
        round: 1,
        status: NodeStatus::Done,
        session_id: Some("persisted-2".into()),
        error: None,
        decision: None,
        started_at: None,
        ended_at: None,
    }]);
    let mut links = Links::new();
    links.insert("persisted-1".into(), Some("wire-1".into()));
    links.insert("persisted-2".into(), Some("wire-2".into()));
    let selected = web(None, Some(vec!["mono-1".into()]));
    let value = reply_of(&selected, runs_of(&w, vec![run.clone()]), &links);
    let run_value = &value["runs"][0];
    let mut run_keys = RUN_KEYS.to_vec();
    run_keys.push("sessions");
    assert_eq!(keys(run_value), sorted(&run_keys));
    assert_eq!(
        keys(&run_value["caller"]),
        sorted(&["kind", "monoId", "triggerNode", "item"])
    );
    assert_eq!(
        keys(&run_value["caller"]["item"]),
        sorted(&["source", "kind", "ref", "title", "url", "repo"])
    );
    assert_eq!(keys(&run_value["summary"]), sorted(&["title", "text"]));
    let node_value = &run_value["nodes"][0];
    let mut node_keys = NODE_KEYS.to_vec();
    node_keys.retain(|k| *k != "output" || node_value.get("output").is_some());
    node_keys.extend(["title", "type", "sessions"]);
    assert_eq!(keys(node_value), sorted(&node_keys));
    assert_eq!(keys(&node_value["decision"]), sorted(&["value", "reason"]));
    assert_eq!(node_value["sessions"], json!(["wire-2", "wire-1"]));
    assert_eq!(run_value["sessions"], json!(["wire-2", "wire-1"]));
    assert_eq!(CALLER_KEYS, ["kind", "monoId", "triggerNode"]);
    // Nothing local or internal leaks, wherever it sits.
    let text = value.to_string();
    for secret in [
        "/Users/me",
        "reuse",
        "persisted-",
        "localPath",
        "token",
        "/work/a",
    ] {
        assert!(!text.contains(secret), "{secret} reached the device");
    }
}

#[test]
fn a_mono_the_device_did_not_select_is_not_named_as_the_caller() {
    let w = workflow("w", &["/work/a"]);
    let mut run = run_of(&w, &[]);
    run.caller = RunCaller::mono("mono-1", "manual");
    let caller = |device: &DeviceContext| {
        reply_of(device, runs_of(&w, vec![run.clone()]), &Links::new())["runs"][0]["caller"].clone()
    };
    let named = caller(&web(None, Some(vec!["mono-1".into()])));
    assert_eq!(named["monoId"], "mono-1");
    assert_eq!(named["kind"], "mono");
    let hidden = caller(&web(None, Some(vec!["other".into()])));
    assert!(hidden.get("monoId").is_none());
    assert_eq!(hidden["kind"], "mono");
    assert_eq!(caller(&web(None, None))["monoId"], "mono-1");
}

#[test]
fn run_sessions_are_deduplicated_even_when_not_adjacent() {
    let w = workflow("w", &["/work/a"]);
    let run = run_of(&w, &[("s0", "p1"), ("s0", "p2"), ("s0", "p1")]);
    let mut links = Links::new();
    links.insert("p1".into(), Some("w1".into()));
    links.insert("p2".into(), Some("w2".into()));
    let value = reply_of(&web(None, None), runs_of(&w, vec![run]), &links);
    assert_eq!(value["runs"][0]["sessions"], json!(["w1", "w2"]));
}

fn text_of(len: usize) -> String {
    "é".repeat(len)
}

#[test]
fn every_string_is_capped_and_the_reply_stays_under_the_budget() {
    let big = text_of(10_000);
    let mut w = workflow("w", &["/work/a"]);
    w.nodes[0].title = Some(big.clone());
    let mut run = run_of(&w, &[("s0", "p1")]);
    run.summary = Some(RunSummary {
        title: big.clone(),
        text: big.clone(),
    });
    run.error = Some(big.clone());
    run.caller = RunCaller::new(CallerKind::Event, big.clone());
    run.caller.item = Json::parse(&json!({"title": big, "url": big}).to_string());
    run.nodes[0].branch = big.clone();
    run.nodes[0].error = Some(big.clone());
    run.nodes[0].decision = Some(Decision {
        value: big.clone(),
        reason: Some(big.clone()),
    });
    let value = reply_of(&web(None, None), runs_of(&w, vec![run]), &Links::new());
    let run_value = &value["runs"][0];
    let cut = |v: &Value, max: usize| {
        let s = v.as_str().unwrap();
        assert!(s.ends_with("[truncated]"), "{s:.20}");
        assert!(s.len() <= max + 20, "{} > {max}", s.len());
    };
    cut(&run_value["summary"]["title"], 256);
    cut(&run_value["summary"]["text"], 8 * 1024);
    cut(&run_value["error"], 4 * 1024);
    cut(&run_value["caller"]["triggerNode"], 256);
    cut(&run_value["caller"]["item"]["title"], 256);
    cut(&run_value["caller"]["item"]["url"], 256);
    let node = &run_value["nodes"][0];
    cut(&node["title"], 256);
    cut(&node["branch"], 256);
    cut(&node["error"], 4 * 1024);
    cut(&node["decision"]["reason"], 4 * 1024);
    cut(&node["decision"]["value"], 256);
}

fn big_run(id: &str, nodes: usize, output_len: usize) -> Value {
    let node = |n: usize| {
        json!({
            "nodeId": format!("n{n}"),
            "status": "done",
            "output": {"a": "x".repeat(output_len)},
        })
    };
    json!({
        "id": id,
        "status": "done",
        "nodes": (0..nodes).map(node).collect::<Vec<_>>(),
    })
}

#[test]
fn the_oldest_outputs_go_first_and_only_those_runs_are_marked() {
    let mut runs = vec![big_run("new", 1, 4_000), big_run("old", 1, 4_000)];
    bound_runs(&mut runs, usize::MAX);
    assert!(runs[0].get("outputsDropped").is_none());
    assert!(runs[1].get("outputsDropped").is_none());

    // A budget a little under the whole: the older run's outputs go, which is enough.
    let total = serde_json::to_vec(&runs).unwrap().len();
    bound_runs(&mut runs, total - 100);
    assert!(runs[0]["nodes"][0].get("output").is_some());
    assert!(runs[1]["nodes"][0].get("output").is_none());
    assert_eq!(runs[1]["outputsDropped"], true);
    assert!(runs[0].get("outputsDropped").is_none());
    assert_eq!(runs[1]["id"], "old");

    // A run with no output to lose is not marked.
    let mut runs = vec![big_run("a", 1, 4_000), json!({"id": "b", "nodes": []})];
    let total = serde_json::to_vec(&runs).unwrap().len();
    bound_runs(&mut runs, total - 100);
    assert_eq!(runs.len(), 2);
    assert!(runs[1].get("outputsDropped").is_none());
}

#[test]
fn the_budget_is_hard_the_oldest_runs_go_then_the_newest_runs_tail() {
    let budget = 20_000;
    let size = |runs: &[Value]| serde_json::to_vec(runs).unwrap().len();

    // Even without outputs the runs do not fit: the oldest are dropped, the newest stays.
    let mut runs: Vec<Value> = (0..30)
        .map(|n| big_run(&format!("r{n}"), 40, 100))
        .collect();
    bound_runs(&mut runs, budget);
    assert!(size(&runs) <= budget);
    assert!(!runs.is_empty());
    assert_eq!(runs[0]["id"], "r0");
    assert!(runs.len() < 30);

    // One run whose steps alone are too many loses its tail.
    let mut runs = vec![big_run("only", 2_000, 10)];
    bound_runs(&mut runs, budget);
    assert!(size(&runs) <= budget);
    assert_eq!(runs[0]["nodesTruncated"], true);
    assert_eq!(runs[0]["nodes"][0]["nodeId"], "n0");
    assert!(runs[0]["nodes"].as_array().unwrap().len() < 2_000);
}

#[test]
fn a_real_reply_of_huge_runs_fits_one_mebibyte() {
    let w = workflow("w", &["/work/a"]);
    let big = text_of(10_000);
    let runs: Vec<WorkflowRun> = (0..20)
        .map(|n| {
            let mut run = run_of(&w, &[]);
            run.id = format!("run-{n}");
            run.nodes = (0..60)
                .map(|i| {
                    let mut node = NodeRun::new(format!("n{i}"), "");
                    node.status = NodeStatus::Done;
                    node.error = Some(big.clone());
                    node.output = Json::parse(&json!({"stdout": big, "stderr": big}).to_string())
                        .and_then(|j| match j {
                            Json::Object(map) => Some(map),
                            _ => None,
                        });
                    node
                })
                .collect();
            run
        })
        .collect();
    let value = reply_of(&web(None, None), runs_of(&w, runs), &Links::new());
    assert!(serde_json::to_vec(&value).unwrap().len() <= 1024 * 1024);
    assert_eq!(value["runs"][0]["id"], "run-0");
    // Runs left out are counted, and the ones kept are not marked as having lost outputs for it.
    let kept = value["runs"].as_array().unwrap().len();
    assert!(kept < 20);
    assert_eq!(value["runsDropped"], 20 - kept);
}

#[test]
fn runs_dropped_is_absent_when_every_run_fits() {
    let w = workflow("w", &["/work/a"]);
    let value = reply_of(
        &web(None, None),
        runs_of(&w, vec![run_of(&w, &[])]),
        &Links::new(),
    );
    assert!(value.get("runsDropped").is_none());
}

// --- the background read ---

fn temp_dir() -> WorkflowsDir {
    let dir = std::env::temp_dir().join(format!(
        "monocode-wf-mobile-{}",
        monocode_core::mono::new_id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    WorkflowsDir::new(dir)
}

fn save(dir: &WorkflowsDir, workflows: Vec<Workflow>) {
    update_workflows(dir, |_| Ok((workflows.clone(), ()))).unwrap();
}

fn save_runs(dir: &WorkflowsDir, id: &str, runs: Vec<WorkflowRun>) {
    update_runs(dir, id, |_| Ok((runs.clone(), ()))).unwrap();
}

fn run_with(w: &Workflow, id: &str, started: u64, snapshot: &Workflow) -> WorkflowRun {
    let mut run = run_of(w, &[]);
    run.id = id.into();
    run.started_at = started;
    run.definition = snapshot.clone();
    run
}

const ID_A: &str = "wf-aaaaaaaa";
const ID_B: &str = "wf-bbbbbbbb";
const ID_GONE: &str = "wf-cccccccc";

fn expect_runs(loaded: Result<Loaded, &'static str>) -> Option<(Workflow, Vec<WorkflowRun>)> {
    match loaded {
        Ok(Loaded::Runs(runs)) => runs,
        _ => panic!("expected a runs read"),
    }
}

#[test]
fn a_hidden_workflow_and_a_missing_one_get_the_same_reply_and_their_runs_are_never_read() {
    let dir = temp_dir();
    let known = vec!["/work/a".to_owned(), "/work/b".to_owned()];
    let device = web(Some(vec![project_id("/work/a")]), None);
    let (visible, hidden) = (workflow(ID_A, &["/work/a"]), workflow(ID_B, &["/work/b"]));
    save(&dir, vec![visible.clone(), hidden.clone()]);
    // The hidden workflow's runs file cannot be read: reading it would show as an error.
    std::fs::create_dir_all(dir.root().join("runs")).unwrap();
    std::fs::write(
        dir.root().join("runs").join(format!("{ID_B}.json")),
        "not json",
    )
    .unwrap();

    let hidden_reply = reply_of(
        &device,
        read_files(&dir, Some(ID_B), &device, &known).unwrap(),
        &Links::new(),
    );
    let missing_reply = reply_of(
        &device,
        read_files(&dir, Some(ID_GONE), &device, &known).unwrap(),
        &Links::new(),
    );
    assert_eq!(hidden_reply, missing_reply);
    assert_eq!(hidden_reply, json!({"workflow": null, "runs": []}));

    // The list names only the visible one.
    let Ok(Loaded::List(list)) = read_files(&dir, None, &device, &known) else {
        panic!("expected a list");
    };
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0.id, ID_A);
}

#[test]
fn a_runs_file_that_cannot_be_read_is_an_error_without_a_path_and_not_never_run() {
    // A narrowed device and one with no selection classify the same file the same way.
    let dir = temp_dir();
    let known = vec!["/work/a".to_owned()];
    let device = web(None, None);
    save(&dir, vec![workflow(ID_A, &["/work/a"])]);
    std::fs::create_dir_all(dir.root().join("runs")).unwrap();
    std::fs::write(
        dir.root().join("runs").join(format!("{ID_A}.json")),
        "not json",
    )
    .unwrap();

    let Err(error) = read_files(&dir, Some(ID_A), &device, &known) else {
        panic!("a runs file that cannot be read is an error");
    };
    assert!(!error.contains('/') && !error.contains("wf-"), "{error}");

    let Ok(Loaded::List(list)) = read_files(&dir, None, &device, &known) else {
        panic!("expected a list");
    };
    assert!(matches!(list[0].1, LastRun::Unreadable));
    // A narrowed device sees the same.
    let narrowed = web(Some(vec![project_id("/work/a")]), None);
    let Ok(Loaded::List(list)) = read_files(&dir, None, &narrowed, &known) else {
        panic!("expected a list");
    };
    assert!(matches!(list[0].1, LastRun::Unreadable));

    // No file at all is "never run".
    let empty = temp_dir();
    save(&empty, vec![workflow(ID_A, &["/work/a"])]);
    let Ok(Loaded::List(list)) = read_files(&empty, None, &device, &known) else {
        panic!("expected a list");
    };
    assert!(matches!(list[0].1, LastRun::Never));
}

#[test]
fn a_run_whose_snapshot_used_a_project_outside_the_selection_is_dropped_but_the_workflow_shows() {
    let dir = temp_dir();
    let known = vec!["/work/a".to_owned(), "/work/b".to_owned()];
    let narrowed = web(Some(vec![project_id("/work/a")]), None);
    let current = workflow(ID_A, &["/work/a"]);
    let older = workflow(ID_A, &["/work/b"]);
    save(&dir, vec![current.clone()]);
    // Newest first: the newest run used project b (not selected), the older one project a.
    save_runs(
        &dir,
        ID_A,
        vec![
            run_with(&current, "run-new", 3_000, &older),
            run_with(&current, "run-old", 1_000, &current),
        ],
    );

    let (workflow, runs) =
        expect_runs(read_files(&dir, Some(ID_A), &narrowed, &known)).expect("the workflow shows");
    assert_eq!(workflow.id, ID_A);
    assert_eq!(
        runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["run-old"]
    );

    // lastRun is the newest run the device may see, not the newest run.
    let Ok(Loaded::List(list)) = read_files(&dir, None, &narrowed, &known) else {
        panic!("expected a list");
    };
    match &list[0].1 {
        LastRun::Seen(_, started) => assert_eq!(*started, 1_000),
        _ => panic!("the visible run is the last run"),
    }
    // A device with no selection sees the newest.
    let all = web(None, None);
    let Ok(Loaded::List(list)) = read_files(&dir, None, &all, &known) else {
        panic!("expected a list");
    };
    assert!(matches!(&list[0].1, LastRun::Seen(_, 3_000)));
}

#[test]
fn a_device_that_sees_none_of_the_projects_sees_nothing() {
    let dir = temp_dir();
    let known = vec!["/work/a".to_owned()];
    save(&dir, vec![workflow(ID_A, &["/work/a"])]);
    let none = web(Some(vec![]), None);
    assert!(expect_runs(read_files(&dir, Some(ID_A), &none, &known)).is_none());
    let Ok(Loaded::List(list)) = read_files(&dir, None, &none, &known) else {
        panic!("expected a list");
    };
    assert!(list.is_empty());
}

#[test]
fn the_workspace_project_and_existing_folders_count_for_visibility() {
    use monocode_core::mono::workflows::model::{Checkout, Workspace};
    let known = vec!["/work/a".to_owned(), "/work/b".to_owned()];
    let only_a = web(Some(vec![project_id("/work/a")]), None);
    // A literal workspace project the device does not see hides the workflow.
    let mut w = workflow("w", &["/work/a"]);
    w.workspace = Workspace {
        project: "/work/b".into(),
        ..Default::default()
    };
    assert!(!workflow_visible(&only_a, &known, &w));
    w.workspace.project = "/work/a".into();
    assert!(workflow_visible(&only_a, &known, &w));
    // A workspace project from a variable, or an input without a value, cannot be told.
    w.workspace.project = "{{s0.path}}".into();
    assert!(!workflow_visible(&only_a, &known, &w));
    w.workspace.project = "{{inputs.nope}}".into();
    assert!(!workflow_visible(&only_a, &known, &w));
    // A step sent to an existing folder: only inside a project (or its worktrees) the device sees.
    let mut w = workflow("w", &["/work/a"]);
    let set = |w: &mut Workflow, path: &str| {
        if let NodeKind::Session { checkout, .. } = &mut w.nodes[0].kind {
            *checkout = Checkout::Existing(path.into());
        }
    };
    set(&mut w, "/work/a-worktrees/feature");
    assert!(workflow_visible(&only_a, &known, &w));
    set(&mut w, "/work/b-worktrees/feature");
    assert!(!workflow_visible(&only_a, &known, &w));
    set(&mut w, "/elsewhere/tree");
    assert!(!workflow_visible(&only_a, &known, &w));
    set(&mut w, "{{inputs.p0}}/x");
    assert!(!workflow_visible(&only_a, &known, &w));
}

#[test]
fn a_run_worktree_path_never_reaches_a_device_but_its_branch_may() {
    use monocode_core::mono::workflows::model::RunWorktree;
    let w = workflow("w", &["/work/a"]);
    let mut run = run_of(&w, &[("s0", "persisted-1")]);
    let tree = |path: &str, branch: &str| RunWorktree {
        path: path.into(),
        branch: branch.into(),
        base: "abc".into(),
        removed_at: None,
    };
    run.worktree = Some(tree("/work/a-worktrees/wf-x-1", "wf/x-1"));
    run.branch_worktrees.insert(
        "each#0".into(),
        tree("/work/a-worktrees/wf-x-1-each-0", "wf/x-1-each-0"),
    );
    run.summary = Some(RunSummary {
        title: "Done".into(),
        text: "Worked in /work/a-worktrees/wf-x-1-each-0 on wf/x-1".into(),
    });
    run.error = Some("failed in /work/a-worktrees/wf-x-1".into());
    run.nodes[0].output = Json::parse(
        r#"{"note":"see /work/a-worktrees/wf-x-1/src","branch":"wf/x-1"}"#,
    )
    .and_then(|j| match j {
        Json::Object(map) => Some(map),
        _ => None,
    });
    let value = reply_of(&web(None, None), runs_of(&w, vec![run]), &Links::new());
    let text = value.to_string();
    assert!(!text.contains("a-worktrees"), "{text}");
    assert!(!text.contains("\"worktree\""), "{text}");
    assert!(
        text.contains("wf/x-1"),
        "a branch name is not a path: {text}"
    );
    // The allow-list of a run did not grow.
    assert!(!RUN_KEYS.contains(&"worktree") && !RUN_KEYS.contains(&"branchWorktrees"));
}

#[test]
fn only_whole_paths_are_hidden_and_the_disk_spelling_counts() {
    use monocode_core::mono::workflows::model::RunWorktree;
    let dir = std::env::temp_dir().join(format!("wf-hide-{}", monocode_core::mono::new_id()));
    let tree = dir.join("app-worktrees").join("wf-x-1");
    std::fs::create_dir_all(&tree).unwrap();
    let real = tree.canonicalize().unwrap();
    let w = workflow("w", &["/work/a"]);
    let mut run = run_of(&w, &[]);
    // Recorded through the symlinked spelling; text may carry the real one (or the reverse).
    run.worktree = Some(RunWorktree {
        path: tree.to_string_lossy().into_owned(),
        branch: "wf/x-1".into(),
        base: "abc".into(),
        removed_at: None,
    });
    let longer = format!("{}-each-0", real.display());
    run.summary = Some(RunSummary {
        title: "t".into(),
        text: format!(
            "in {} and {} and {longer} and x{}",
            real.display(),
            tree.display(),
            real.display()
        ),
    });
    let value = reply_of(&web(None, None), runs_of(&w, vec![run]), &Links::new());
    let text = value["runs"][0]["summary"]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!text.contains(&format!("{} ", real.display())), "{text}");
    assert_eq!(text.matches("(worktree)").count(), 2, "{text}");
    // A longer path that merely starts with it, and a path glued to other text, stay as written.
    assert!(text.contains(&longer), "{text}");
    assert!(text.contains(&format!("x{}", real.display())), "{text}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_path_ending_a_sentence_is_hidden() {
    use super::workflows::replace_path_for_test as hide;
    let path = "/x/app-worktrees/wf-a-1";
    assert_eq!(hide(&format!("in {path}."), path), "in (worktree).");
    assert_eq!(
        hide(&format!("in {path}, then"), path),
        "in (worktree), then"
    );
    assert_eq!(
        hide(&format!("in {path}. Next"), path),
        "in (worktree). Next"
    );
    assert_eq!(hide(&format!("{path}.txt"), path), format!("{path}.txt"));
}
