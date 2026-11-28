//! The Workflows reads of the web app: who sees which workflow, and which sessions a run links.
use super::workflows::{Loaded, bound_runs, workflow_visible};
use super::*;
use monocode_core::mobile::{DeviceKind, Permission};
use monocode_core::mono::workflows::model::{
    CallerKind, Checkout, InputType, InputValue, Node, NodeKind, NodeRun, NodeStatus, RunCaller,
    RunStatus, RunSummary, Workflow, WorkflowInput, WorkflowRun,
};

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
            harness: None,
            model: None,
            effort: None,
            reuse_session: None,
            nest_under: None,
            timeout_minutes: None,
            target: None,
            account: None,
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

            // Both projects: the workflow is visible and links the sessions that exist.
            let both = web(Some(vec![project_id(&path_a), project_id(&path_b)]), None);
            let reply =
                shell.workflows_reply(&both, Loaded::Runs(Some(w.clone()), vec![run.clone()]), cx);
            assert_eq!(sessions_of(&reply), vec![wire_a.clone(), wire_b.clone()]);
            assert_eq!(reply["runs"][0]["durationMs"], 3_000);
            assert_eq!(reply["runs"][0]["summary"]["text"], "All good");
            assert_eq!(
                reply["runs"][0]["nodes"][0]["sessions"],
                json!([wire_a.clone()])
            );

            // Only the workflow's project: b's session is not linked.
            let just_a = web(Some(vec![project_id(&path_a)]), None);
            let reply = shell.workflows_reply(
                &just_a,
                Loaded::Runs(Some(w.clone()), vec![run.clone()]),
                cx,
            );
            assert_eq!(sessions_of(&reply), vec![wire_a.clone()]);

            // A phone and a device with no selection see them all.
            let reply = shell.workflows_reply(
                &web(None, None),
                Loaded::Runs(Some(w.clone()), vec![run.clone()]),
                cx,
            );
            assert_eq!(sessions_of(&reply), vec![wire_a.clone(), wire_b.clone()]);

            // A device that sees only the other project sees nothing, not even the name.
            let just_b = web(Some(vec![project_id(&path_b)]), None);
            let reply = shell.workflows_reply(
                &just_b,
                Loaded::Runs(Some(w.clone()), vec![run.clone()]),
                cx,
            );
            assert_eq!(reply, json!({"workflow": null, "runs": []}));
            let reply = shell.workflows_reply(
                &web(Some(vec![]), None),
                Loaded::Runs(Some(w.clone()), vec![run]),
                cx,
            );
            assert_eq!(reply, json!({"workflow": null, "runs": []}));
            // A workflow that does not exist looks the same.
            let reply = shell.workflows_reply(&both, Loaded::Runs(None, Vec::new()), cx);
            assert_eq!(reply, json!({"workflow": null, "runs": []}));
        })
        .unwrap();
}

#[gpui::test]
fn the_list_hides_workflows_over_projects_the_device_cannot_see(cx: &mut gpui::TestAppContext) {
    let (window, a, b) = super::super::tests::window(cx);
    window
        .update(cx, |shell, _, cx| {
            let (path_a, path_b) = (project_of(shell, a, cx), project_of(shell, b, cx));
            let mut mine = workflow("mine", &[&path_a]);
            mine.enabled = true;
            mine.next_run_at = Some(9_000);
            let shared = workflow("shared", &[&path_a, &path_b]);
            let list = |shell: &mut Shell, device: &DeviceContext, cx: &App| {
                let loaded = Loaded::List(vec![
                    (mine.clone(), Some((json!("done"), 5))),
                    (shared.clone(), None),
                ]);
                let reply = shell.workflows_reply(device, loaded, cx);
                reply["workflows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| w["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            };
            let just_a = web(Some(vec![project_id(&path_a)]), None);
            assert_eq!(list(shell, &just_a, cx), vec!["mine"]);
            assert!(list(shell, &web(Some(vec![project_id(&path_b)]), None), cx).is_empty());
            assert!(list(shell, &web(Some(vec![]), None), cx).is_empty());
            assert_eq!(list(shell, &web(None, None), cx), vec!["mine", "shared"]);

            let reply = shell.workflows_reply(
                &just_a,
                Loaded::List(vec![(mine.clone(), Some((json!("done"), 5)))]),
                cx,
            );
            let entry = &reply["workflows"][0];
            assert_eq!(entry["enabled"], true);
            assert_eq!(entry["nextRunAt"], 9_000);
            assert_eq!(entry["lastRun"], json!({"status": "done", "startedAt": 5}));
            assert_eq!(entry["triggers"], json!([]));
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
                let reply = shell.workflows_reply(
                    device,
                    Loaded::Runs(Some(w.clone()), vec![run.clone()]),
                    cx,
                );
                reply["runs"][0]["sessions"].clone()
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

#[test]
fn long_values_are_cut_and_the_oldest_outputs_go_first_when_the_reply_is_too_big() {
    let big = "é".repeat(10_000);
    let run = |id: &str| {
        json!({
            "id": id,
            "error": big,
            "summary": {"title": "t", "text": big},
            "nodes": [{"nodeId": "n", "error": big, "output": {"a": big, "b": [big]}}],
        })
    };
    let mut runs = vec![run("new"), run("old")];
    bound_runs(&mut runs, usize::MAX);
    let text = runs[0]["nodes"][0]["output"]["a"].as_str().unwrap();
    assert!(text.ends_with("[truncated]") && text.len() < 4_200);
    assert!(
        runs[0]["nodes"][0]["error"]
            .as_str()
            .unwrap()
            .ends_with("[truncated]")
    );
    assert!(runs[0]["summary"]["text"].as_str().unwrap().len() < 8_300);
    assert!(runs[0].get("outputsDropped").is_none());

    // A budget a little under the whole drops the older run's outputs, which is enough.
    let total = serde_json::to_vec(&runs).unwrap().len();
    bound_runs(&mut runs, total - 100);
    assert!(runs[0]["nodes"][0].get("output").is_some());
    assert!(runs[1]["nodes"][0].get("output").is_none());
    assert_eq!(runs[1]["outputsDropped"], true);
    assert!(runs[0].get("outputsDropped").is_none());
    // The run itself, its status and its sessions survive.
    assert_eq!(runs[1]["id"], "old");
}
