//! The workspace and concurrency of a workflow, the new session and command fields, and what
//! validation says about them: defaults, the file shape, the checkout rules (cloud steps, another
//! project) and the non-blocking warnings.
use super::engine::{run_scope, start_run};
use super::fixtures::workflow_from;
use super::model::*;
use super::models::ModelCatalog;
use super::ordered::Json;
use super::template::render_template;
use super::validate::{validate_workflow, workflow_warnings};
use super::yaml::{export_workflow, finish_import, preview_import};
use serde_json::{json, Value};

/// A workflow with the given nodes and edges (and extra top-level fields), a project input and a
/// manual trigger `start`.
fn flow(extra: Value, nodes: Vec<Value>, edges: Vec<Value>) -> Workflow {
    let mut all = vec![json!({"id": "start", "type": "manual"})];
    all.extend(nodes);
    let mut spec = json!({
        "id": "wf-spaces01", "name": "Spaces", "enabled": false, "createdAt": 1,
        "inputs": {"project": {"type": "project"}, "other": {"type": "project"}},
        "inputValues": {"project": "/code/app", "other": "/code/lib"},
        "nodes": all, "edges": edges
    });
    for (k, v) in extra.as_object().unwrap() {
        spec[k] = v.clone();
    }
    workflow_from(&spec.to_string())
}

fn session(id: &str, extra: Value) -> Value {
    let mut node = json!({"id": id, "type": "session", "prompt": "go"});
    for (k, v) in extra.as_object().unwrap() {
        node[k] = v.clone();
    }
    node
}

fn command(id: &str, extra: Value) -> Value {
    let mut node = json!({"id": id, "type": "command", "command": "true"});
    for (k, v) in extra.as_object().unwrap() {
        node[k] = v.clone();
    }
    node
}

fn chain(ids: &[&str]) -> Vec<Value> {
    let mut edges = vec![json!({"from": "start", "to": ids[0]})];
    for pair in ids.windows(2) {
        edges.push(json!({"from": pair[0], "to": pair[1]}));
    }
    edges
}

const PER_RUN: fn() -> Value =
    || json!({"workspace": {"mode": "perRun", "project": "{{inputs.project}}"}});

fn issues(w: &Workflow) -> Vec<String> {
    validate_workflow(w)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

fn warnings(w: &Workflow) -> Vec<String> {
    workflow_warnings(w)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

fn session_kind(w: &Workflow, id: &str) -> (String, Checkout) {
    match &w.node(id).unwrap().kind {
        NodeKind::Session {
            project, checkout, ..
        } => (project.clone(), checkout.clone()),
        other => panic!("not a session: {other:?}"),
    }
}

// ---- defaults and the file shape ------------------------------------------------------------

#[test]
fn a_workflow_without_them_has_a_per_run_workspace_and_three_runs_at_once() {
    let w = flow(
        json!({}),
        vec![session(
            "a",
            json!({"project": "{{inputs.project}}", "checkout": "root"}),
        )],
        chain(&["a"]),
    );
    assert_eq!(w.workspace.mode, WorkspaceMode::PerRun);
    assert_eq!(w.workspace.project, "");
    assert_eq!(w.workspace.base(), "main");
    assert_eq!(w.concurrency.limit(), 3);
    // Defaults are not written.
    let written: Value = serde_json::to_value(&w).unwrap();
    assert!(written.get("workspace").is_none() && written.get("concurrency").is_none());
}

#[test]
fn a_session_without_checkout_or_project_uses_the_run_worktree_of_the_workspace_project() {
    let w = flow(PER_RUN(), vec![session("a", json!({}))], chain(&["a"]));
    assert_eq!(session_kind(&w, "a"), (String::new(), Checkout::Run));
    assert_eq!(issues(&w), Vec::<String>::new());
    // The default is written as `run`, and an empty project is not written at all.
    let written: Value = serde_json::to_value(w.node("a").unwrap()).unwrap();
    assert_eq!(written["checkout"], json!("run"));
    assert!(written.get("project").is_none());
}

#[test]
fn the_workspace_concurrency_and_new_step_fields_survive_a_round_trip() {
    let w = flow(
        json!({
            "workspace": {"mode": "none", "project": "/code/app", "baseBranch": "develop"},
            "concurrency": {"max": 7}
        }),
        vec![
            session(
                "a",
                json!({"checkout": {"existing": "/code/app-worktrees/mine"}}),
            ),
            command("b", json!({"onFailure": "continue"})),
        ],
        chain(&["a", "b"]),
    );
    let text = serde_json::to_string(&w).unwrap();
    let back: Workflow = serde_json::from_str(&text).unwrap();
    assert_eq!(back, w);
    assert_eq!(back.workspace.mode, WorkspaceMode::None);
    assert_eq!(back.workspace.base_branch.as_deref(), Some("develop"));
    assert_eq!(back.concurrency.limit(), 7);
    assert_eq!(
        session_kind(&back, "a").1,
        Checkout::Existing("/code/app-worktrees/mine".into())
    );
    match &back.node("b").unwrap().kind {
        NodeKind::Command { on_failure, .. } => assert_eq!(on_failure.as_deref(), Some("continue")),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_yaml_export_and_import_keep_the_workspace_concurrency_and_new_fields() {
    let w = flow(
        json!({
            "workspace": {"mode": "perRun", "project": "{{inputs.project}}", "baseBranch": "develop"},
            "concurrency": {"max": 2}
        }),
        vec![
            session("a", json!({"checkout": "run"})),
            command("b", json!({"onFailure": "continue"})),
        ],
        chain(&["a", "b"]),
    );
    let yaml = export_workflow(&w);
    assert!(
        yaml.contains("workspace:") && yaml.contains("baseBranch: develop"),
        "{yaml}"
    );
    assert!(
        yaml.contains("concurrency:") && yaml.contains("onFailure: continue"),
        "{yaml}"
    );
    let preview = preview_import(&yaml, &ModelCatalog::unprobed()).unwrap();
    let mut values = super::ordered::OrderedMap::new();
    values.insert("project", InputValue::String("/code/app".into()));
    values.insert("other", InputValue::String("/code/lib".into()));
    let imported = finish_import(&preview, &values, 5).unwrap();
    assert_eq!(imported.workspace, w.workspace);
    assert_eq!(imported.concurrency, w.concurrency);
    assert_eq!(imported.nodes, w.nodes);
}

#[test]
fn a_file_with_a_workspace_that_cannot_be_read_is_refused() {
    let yaml = "monoptahWorkflow: 1\nname: x\nworkspace:\n  mode: everywhere\nnodes:\n  - id: start\n    type: manual\nedges: []\n";
    let error = preview_import(yaml, &ModelCatalog::unprobed()).unwrap_err();
    assert!(error.contains("workspace"), "{error}");
}

// ---- validation -----------------------------------------------------------------------------

#[test]
fn a_step_without_a_project_needs_the_workspace_to_name_one() {
    let w = flow(
        json!({}),
        vec![session("a", json!({"checkout": "root"}))],
        chain(&["a"]),
    );
    assert!(
        issues(&w).iter().any(|m| m.contains("project is not set")),
        "{:?}",
        issues(&w)
    );
    let named = flow(
        PER_RUN(),
        vec![session("a", json!({"checkout": "root"}))],
        chain(&["a"]),
    );
    assert_eq!(issues(&named), Vec::<String>::new());
}

#[test]
fn the_workspace_project_is_a_path_or_a_declared_project_input() {
    for project in ["/code/app", "app"] {
        let w = flow(
            json!({"workspace": {"mode": "perRun", "project": project}}),
            vec![session("a", json!({}))],
            chain(&["a"]),
        );
        assert_eq!(issues(&w), Vec::<String>::new(), "{project}");
    }
    for (project, phrase) in [
        ("{{inputs.nope}}", "is not a declared project input"),
        (
            "{{inputs.project}}/x",
            "workspace.project must be a project path or exactly",
        ),
    ] {
        let w = flow(
            json!({"workspace": {"mode": "perRun", "project": project}}),
            vec![session("a", json!({}))],
            chain(&["a"]),
        );
        assert!(
            issues(&w).iter().any(|m| m.contains(phrase)),
            "{project}: {:?}",
            issues(&w)
        );
    }
}

#[test]
fn concurrency_and_the_base_branch_are_checked() {
    for max in [0.0, 11.0, 2.5] {
        let mut w = flow(PER_RUN(), vec![session("a", json!({}))], chain(&["a"]));
        w.concurrency = Concurrency { max };
        assert!(
            issues(&w)
                .iter()
                .any(|m| m.contains("concurrency.max must be a whole number from 1 to 10")),
            "{max}"
        );
    }
    for max in [1.0, 3.0, 10.0] {
        let mut w = flow(PER_RUN(), vec![session("a", json!({}))], chain(&["a"]));
        w.concurrency = Concurrency { max };
        assert_eq!(issues(&w), Vec::<String>::new(), "{max}");
    }
    for branch in ["", "-x", "a..b", "a b", "release/", "{{inputs.project}}"] {
        let mut w = flow(PER_RUN(), vec![session("a", json!({}))], chain(&["a"]));
        w.workspace.base_branch = Some(branch.into());
        assert!(
            issues(&w)
                .iter()
                .any(|m| m.contains("workspace.baseBranch")),
            "{branch:?}"
        );
    }
    let mut ok = flow(PER_RUN(), vec![session("a", json!({}))], chain(&["a"]));
    ok.workspace.base_branch = Some("release/2.0".into());
    assert_eq!(issues(&ok), Vec::<String>::new());
}

#[test]
fn a_cloud_step_cannot_use_the_run_worktree_or_an_existing_folder() {
    for checkout in [json!("run"), json!({"existing": "/code/app-worktrees/x"})] {
        let w = flow(
            PER_RUN(),
            vec![session(
                "a",
                json!({"checkout": checkout, "target": {"kind": "codespace"}}),
            )],
            chain(&["a"]),
        );
        let found = issues(&w);
        assert!(
            found.iter().any(|m| m.contains("A cloud session")),
            "{checkout}: {found:?}"
        );
    }
    // A cloud step that forgets its checkout gets the default, which is refused with the way out.
    let forgot = flow(
        PER_RUN(),
        vec![session("a", json!({"target": {"kind": "codespace"}}))],
        chain(&["a"]),
    );
    assert!(issues(&forgot)
        .iter()
        .any(|m| m.contains("use checkout \"root\"")));
    let root = flow(
        PER_RUN(),
        vec![session(
            "a",
            json!({"checkout": "root", "target": {"kind": "codespace"}}),
        )],
        chain(&["a"]),
    );
    assert_eq!(issues(&root), Vec::<String>::new());
}

#[test]
fn a_step_in_another_project_cannot_use_the_run_worktree() {
    let w = flow(
        PER_RUN(),
        vec![session("a", json!({"project": "{{inputs.other}}"}))],
        chain(&["a"]),
    );
    let found = issues(&w);
    assert!(
        found
            .iter()
            .any(|m| m.contains("checkout run works in the run worktree")),
        "{found:?}"
    );
    // Its own checkout, or the same project, is fine.
    for extra in [
        json!({"project": "{{inputs.other}}", "checkout": "root"}),
        json!({"project": "{{inputs.other}}", "checkout": "newWorktree"}),
        json!({"project": "{{inputs.project}}"}),
    ] {
        let ok = flow(PER_RUN(), vec![session("a", extra.clone())], chain(&["a"]));
        assert_eq!(issues(&ok), Vec::<String>::new(), "{extra}");
    }
    // With no run worktree (mode none) there is nothing to be in the wrong project of.
    let none = flow(
        json!({"workspace": {"mode": "none", "project": "{{inputs.project}}"}}),
        vec![session("a", json!({"project": "{{inputs.other}}"}))],
        chain(&["a"]),
    );
    assert_eq!(issues(&none), Vec::<String>::new());
}

#[test]
fn on_failure_is_fail_or_continue() {
    let bad = flow(
        PER_RUN(),
        vec![command("a", json!({"onFailure": "ignore"}))],
        chain(&["a"]),
    );
    assert!(issues(&bad)
        .iter()
        .any(|m| m.contains("onFailure \"ignore\" is not valid")));
    for value in ["fail", "continue"] {
        let ok = flow(
            PER_RUN(),
            vec![command("a", json!({"onFailure": value}))],
            chain(&["a"]),
        );
        assert_eq!(issues(&ok), Vec::<String>::new(), "{value}");
    }
}

#[test]
fn run_worktree_and_run_branch_are_run_variables() {
    let w = flow(
        PER_RUN(),
        vec![session(
            "a",
            json!({"prompt": "Work in {{run.worktree}} on {{run.branch}}"}),
        )],
        chain(&["a"]),
    );
    assert_eq!(issues(&w), Vec::<String>::new());
    // They hold the run's worktree, and the branch's own inside a For each.
    let mut run = start_run(&w, RunCaller::new(CallerKind::Manual, "start"), 1, "run-1");
    let scope = |run: &WorkflowRun, branch: &str| run_scope(run, branch, Some("a"));
    let text = |run: &WorkflowRun, branch: &str| {
        render_template(
            "{{run.worktree}}|{{run.branch}}",
            &scope(run, branch),
            false,
        )
    };
    assert_eq!(text(&run, ""), "|");
    run.worktree = Some(RunWorktree {
        path: "/wt/run".into(),
        branch: "wf/spaces-1".into(),
        base: "abc".into(),
        removed_at: None,
    });
    assert_eq!(text(&run, ""), "/wt/run|wf/spaces-1");
    run.branch_worktrees.insert(
        "each#1".into(),
        RunWorktree {
            path: "/wt/run-each-1".into(),
            branch: "wf/spaces-1-each-1".into(),
            base: "def".into(),
            removed_at: None,
        },
    );
    assert_eq!(text(&run, "each#1"), "/wt/run-each-1|wf/spaces-1-each-1");
    // Another branch without its own yet reads the run's.
    assert_eq!(text(&run, "each#0"), "/wt/run|wf/spaces-1");
}

// ---- the fingerprint ------------------------------------------------------------------------

#[test]
fn the_fingerprint_covers_the_workspace_and_concurrency() {
    let w = flow(PER_RUN(), vec![session("a", json!({}))], chain(&["a"]));
    let base = definition_fingerprint(&w);
    let mut other_project = w.clone();
    other_project.workspace.project = "{{inputs.other}}".into();
    let mut other_mode = w.clone();
    other_mode.workspace.mode = WorkspaceMode::None;
    let mut other_branch = w.clone();
    other_branch.workspace.base_branch = Some("develop".into());
    let mut other_max = w.clone();
    other_max.concurrency = Concurrency { max: 8.0 };
    for changed in [other_project, other_mode, other_branch, other_max] {
        assert_ne!(definition_fingerprint(&changed), base);
    }
    // The defaults add nothing, and what the app counts with does not count.
    let mut counted = w.clone();
    counted.run_counter = 40;
    counted.stop_run_ids = vec!["run-1".into()];
    assert_eq!(definition_fingerprint(&counted), base);
    let mut default_again = w.clone();
    default_again.concurrency = Concurrency::default();
    assert_eq!(definition_fingerprint(&default_again), base);
}

#[test]
fn a_changed_checkout_or_on_failure_changes_the_fingerprint_too() {
    let a = flow(
        PER_RUN(),
        vec![
            session("a", json!({"checkout": "run"})),
            command("b", json!({})),
        ],
        chain(&["a", "b"]),
    );
    let mut b = a.clone();
    if let NodeKind::Session { checkout, .. } = &mut b.nodes[1].kind {
        *checkout = Checkout::Root;
    }
    assert_ne!(definition_fingerprint(&a), definition_fingerprint(&b));
    let mut c = a.clone();
    if let NodeKind::Command { on_failure, .. } = &mut c.nodes[2].kind {
        *on_failure = Some("continue".into());
    }
    assert_ne!(definition_fingerprint(&a), definition_fingerprint(&c));
}

// ---- the warnings ---------------------------------------------------------------------------

#[test]
fn two_steps_that_can_run_at_once_in_the_run_worktree_are_warned_about() {
    let w = flow(
        PER_RUN(),
        vec![session("a", json!({})), session("b", json!({}))],
        vec![
            json!({"from": "start", "to": "a"}),
            json!({"from": "start", "to": "b"}),
        ],
    );
    // A warning never blocks: the definition has no issue.
    assert_eq!(issues(&w), Vec::<String>::new());
    let found = warnings(&w);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("\"a\" and \"b\" can run at the same time"));
    assert!(found[0].contains("the run worktree"));
}

#[test]
fn steps_in_order_or_with_their_own_worktree_raise_no_warning() {
    let ordered = flow(
        PER_RUN(),
        vec![session("a", json!({})), session("b", json!({}))],
        chain(&["a", "b"]),
    );
    assert_eq!(warnings(&ordered), Vec::<String>::new());
    let own = flow(
        PER_RUN(),
        vec![
            session("a", json!({})),
            session("b", json!({"checkout": "newWorktree"})),
        ],
        vec![
            json!({"from": "start", "to": "a"}),
            json!({"from": "start", "to": "b"}),
        ],
    );
    assert_eq!(warnings(&own), Vec::<String>::new());
}

#[test]
fn a_command_and_a_session_that_can_run_at_once_share_the_run_worktree() {
    let w = flow(
        PER_RUN(),
        vec![session("a", json!({})), command("b", json!({}))],
        vec![
            json!({"from": "start", "to": "a"}),
            json!({"from": "start", "to": "b"}),
        ],
    );
    assert_eq!(warnings(&w).len(), 1);
    // A command that names its own folder does not.
    let rooted = flow(
        PER_RUN(),
        vec![
            session("a", json!({})),
            command("b", json!({"cwd": "root"})),
        ],
        vec![
            json!({"from": "start", "to": "a"}),
            json!({"from": "start", "to": "b"}),
        ],
    );
    assert_eq!(warnings(&rooted), Vec::<String>::new());
}

#[test]
fn the_two_sides_of_an_if_never_run_together() {
    let w = flow(
        PER_RUN(),
        vec![
            json!({"id": "gate", "type": "if", "condition": "{{inputs.project}} == \"x\""}),
            session("yes", json!({})),
            session("no", json!({})),
        ],
        vec![
            json!({"from": "start", "to": "gate"}),
            json!({"from": "gate", "to": "yes", "handle": "true"}),
            json!({"from": "gate", "to": "no", "handle": "false"}),
        ],
    );
    assert_eq!(issues(&w), Vec::<String>::new());
    assert_eq!(warnings(&w), Vec::<String>::new());
}

#[test]
fn steps_of_a_for_each_have_a_worktree_per_item_and_the_loop_does_not_clash_with_the_top() {
    let w = flow(
        json!({
            "workspace": {"mode": "perRun", "project": "{{inputs.project}}"},
            "inputs": {"project": {"type": "project"}, "other": {"type": "project"}, "items": {"type": "string"}},
            "inputValues": {"project": "/code/app", "other": "/code/lib", "items": "x"}
        }),
        vec![
            json!({"id": "each", "type": "forEach", "in": "{{inputs.items}}", "as": "item"}),
            session("inner", json!({"parent": "each"})),
            session("top", json!({})),
        ],
        vec![
            json!({"from": "start", "to": "each"}),
            json!({"from": "start", "to": "top"}),
        ],
    );
    // `inner` works in the worktree of its branch, `top` in the run's: they do not share one.
    assert_eq!(warnings(&w), Vec::<String>::new());
}

#[test]
fn without_a_run_worktree_runs_that_go_at_once_write_into_the_project_folder() {
    let mut w = flow(
        json!({"workspace": {"mode": "none", "project": "{{inputs.project}}"}}),
        vec![session("a", json!({}))],
        chain(&["a"]),
    );
    // The default of three runs at once, in mode none.
    let found = warnings(&w);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("workspace mode none"));
    w.concurrency = Concurrency { max: 1.0 };
    assert_eq!(warnings(&w), Vec::<String>::new());
    // A run worktree per run is the answer.
    let per_run = flow(PER_RUN(), vec![session("a", json!({}))], chain(&["a"]));
    assert_eq!(warnings(&per_run), Vec::<String>::new());
    // Nothing is written by a workflow that only reads.
    let none_without_writers = flow(
        json!({"workspace": {"mode": "none"}}),
        vec![json!({"id": "gate", "type": "if", "condition": "true == true"})],
        chain(&["gate"]),
    );
    assert_eq!(warnings(&none_without_writers), Vec::<String>::new());
}

#[test]
fn per_run_with_no_workspace_project_warns_too_since_no_run_worktree_is_made() {
    // D46: the default mode is perRun, but without a project there is nothing to make a worktree of.
    let mut w = flow(
        json!({"workspace": {"mode": "perRun"}}),
        vec![session("a", json!({}))],
        chain(&["a"]),
    );
    let found = warnings(&w);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("no workspace project"), "{found:?}");
    w.concurrency = Concurrency { max: 1.0 };
    assert_eq!(warnings(&w), Vec::<String>::new());
}

#[test]
fn the_json_of_an_unreadable_workspace_is_left_unread() {
    let spec = json!({
        "id": "wf-spaces01", "name": "x", "nodes": [{"id": "start", "type": "manual"}],
        "workspace": {"mode": 5}
    });
    assert!(super::parse::parse_workflow(&Json::parse(&spec.to_string()).unwrap()).is_none());
}
