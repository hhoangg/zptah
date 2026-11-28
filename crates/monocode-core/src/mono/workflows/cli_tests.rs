//! Port of `agentAppWorkflows.test.ts` (54 tests) against an in-memory host, plus checks of the
//! real store-backed host. The two "only a Mono's own conversation" checks belong to the app's
//! dispatch (they are tested in `monocode-gpui`); the rest keep their names and assertions.
use super::cli::*;
use super::global::WorkflowsDir;
use super::guide::{guide_examples, workflow_guide};
use super::model::*;
use super::models::{CatalogHarness, CatalogModel, ModelCatalog, Probe};
use super::ordered::{Json, OrderedMap};
use super::preflight::{Preflight, RequirementStatus};
use super::store::{load_runs, load_workflows, update_workflows};
use super::validate::validate_workflow;
use crate::provider::Kind;
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

fn now() -> u64 {
    Local
        .with_ymd_and_hms(2026, 10, 22, 10, 0, 0)
        .single()
        .unwrap()
        .timestamp_millis() as u64
}

/// Claude installed with two models; nothing else.
fn catalog() -> ModelCatalog {
    let models = ["claude-sonnet-5", "claude-opus-5"]
        .map(|id| CatalogModel {
            id: id.into(),
            alias_target: None,
            efforts: vec![],
        })
        .to_vec();
    ModelCatalog {
        probed: true,
        harnesses: Kind::ALL
            .into_iter()
            .map(|kind| CatalogHarness {
                id: kind.id().into(),
                installed: kind == Kind::Claude,
                models: (kind == Kind::Claude).then(|| models.clone()),
            })
            .collect(),
    }
}

#[derive(Default)]
struct Memory {
    stored: Vec<Workflow>,
    runs: HashMap<String, Vec<WorkflowRun>>,
    live: HashSet<String>,
    /// How many runs of a workflow go, when more than the one `live` stands for.
    live_n: HashMap<String, usize>,
    /// The queued runs `cancel_queued` was asked to cancel.
    cancelled: Vec<(String, String)>,
    checks: usize,
    updates: usize,
    cleared: Vec<String>,
    /// What `preflight` answers; None finds every requirement met.
    preflight: Option<Preflight>,
    /// The requirement names each `preflight` call was asked about.
    preflights: Vec<Vec<String>>,
    /// The Mono's projects; None accepts a project value as given.
    projects: Option<Vec<String>>,
    /// Account selectors the host judges unusable, with why (the app's accounts are not here).
    bad_accounts: HashMap<String, String>,
    /// The calling Mono; None is the user.
    caller: Option<MonoCaller>,
    /// Ids of workflows the calling Mono had before workflows were global.
    legacy: HashSet<String>,
}

impl WorkflowHost for Memory {
    fn load(&mut self) -> Result<Vec<Workflow>, String> {
        Ok(self.stored.clone())
    }
    fn update<T>(
        &mut self,
        mut edit: impl FnMut(&mut Self, Vec<Workflow>) -> Result<(Vec<Workflow>, T), String>,
    ) -> Result<T, String> {
        self.updates += 1;
        let (next, result) = edit(self, self.stored.clone())?;
        self.stored = next;
        Ok(result)
    }
    fn runs(&mut self, workflow_id: &str) -> Result<Vec<WorkflowRun>, String> {
        Ok(self.runs.get(workflow_id).cloned().unwrap_or_default())
    }
    fn clear_runs(&mut self, workflow_id: &str) {
        self.cleared.push(workflow_id.into());
    }
    fn catalog(&mut self, _: &Workflow) -> ModelCatalog {
        catalog()
    }
    fn is_running(&self, workflow_id: &str) -> bool {
        self.live_runs(workflow_id) > 0
    }
    fn live_runs(&self, workflow_id: &str) -> usize {
        self.live_n
            .get(workflow_id)
            .copied()
            .unwrap_or_else(|| usize::from(self.live.contains(workflow_id)))
    }
    fn cancel_queued(&mut self, workflow_id: &str, run_id: &str) -> Result<bool, String> {
        self.cancelled.push((workflow_id.into(), run_id.into()));
        Ok(true)
    }
    fn request_check_now(&mut self) {
        self.checks += 1;
    }
    fn now(&self) -> u64 {
        now()
    }
    fn preflight(&mut self, w: &Workflow) -> Preflight {
        self.preflights
            .push(w.requires.iter().map(|r| r.name.clone()).collect());
        self.preflight.clone().unwrap_or_else(|| Preflight {
            statuses: w
                .requires
                .iter()
                .map(|r| RequirementStatus {
                    requirement: r.clone(),
                    met: true,
                    detail: None,
                })
                .collect(),
        })
    }
    fn project_path(&self, input: &str) -> Option<String> {
        let projects = self
            .caller
            .as_ref()
            .map(|c| &c.projects)
            .or(self.projects.as_ref());
        match projects {
            None => Some(input.to_string()),
            Some(projects) => crate::mono::resolve_project(projects, input).ok(),
        }
    }
    fn account_problem(&mut self, selector: &str) -> Option<String> {
        self.bad_accounts.get(selector).cloned()
    }
    fn caller(&self) -> Option<&MonoCaller> {
        self.caller.as_ref()
    }
    fn is_legacy_id(&mut self, id: &str) -> bool {
        self.legacy.contains(id)
    }
}

/// A small valid definition: schedule -> session -> report.
fn definition(extra: Value) -> Value {
    let mut v = json!({
        "name": "Ticket review",
        "inputs": {"project": {"type": "project"}},
        "inputValues": {"project": "/code/app"},
        "workspace": {"mode": "perRun", "project": "{{inputs.project}}"},
        "nodes": [
            {"id": "start", "type": "schedule", "schedule": {"kind": "weekdays", "time": "09:00"}},
            {
                "id": "fetch",
                "type": "session",
                "project": "{{inputs.project}}",
                "prompt": "List my open tickets.",
                "checkout": "root",
                "output": {"tickets": {"type": "list"}}
            }
        ],
        "edges": [{"from": "start", "to": "fetch"}]
    });
    for (key, value) in extra.as_object().unwrap() {
        v[key] = value.clone();
    }
    v
}

fn without(v: Value, key: &str) -> Value {
    Value::Object(
        v.as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k != key)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    )
}

fn definition_nodes() -> Vec<Value> {
    definition(json!({}))["nodes"].as_array().unwrap().clone()
}

fn run(id: &str, status: RunStatus, workflow: &str) -> WorkflowRun {
    let mut fetch = NodeRun::new("fetch", "");
    fetch.status = NodeStatus::Done;
    let mut dev = NodeRun::new("dev", "each#0");
    dev.status = NodeStatus::Failed;
    dev.error = Some("boom".into());
    WorkflowRun {
        id: id.into(),
        workflow_id: workflow.into(),
        definition: Workflow::new(workflow, "x"),
        status,
        started_at: now() - 60_000,
        ended_at: None,
        caller: RunCaller::new(CallerKind::Manual, "start"),
        items: BTreeMap::new(),
        nodes: vec![fetch, dev],
        report_posted_at: None,
        error: None,
        cloud_instances: Vec::new(),
        number: None,
        queued_at: None,
        worktree: None,
        branch_worktrees: Default::default(),
        summary: None,
    }
}

struct Fx {
    host: Memory,
}

impl Fx {
    fn new() -> Self {
        Self {
            host: Memory::default(),
        }
    }
    fn call(&mut self, action: &str, input: Value) -> Result<Json, String> {
        handle(action, &Json::from(&input), &mut self.host)
    }
    fn ok(&mut self, action: &str, input: Value) -> Json {
        self.call(action, input)
            .unwrap_or_else(|e| panic!("{action} failed: {e}"))
    }
    fn err(&mut self, action: &str, input: Value) -> String {
        match self.call(action, input) {
            Err(e) => e,
            Ok(v) => panic!("{action} should fail, got {}", v.stringify(false)),
        }
    }
    /// workflows.add of a definition; returns the result.
    fn add(&mut self, workflow: Value) -> Json {
        self.ok("workflows.add", json!({"workflow": workflow}))
    }
    fn stored(&self) -> &[Workflow] {
        &self.host.stored
    }
    fn seed(&mut self, list: Vec<Workflow>) {
        self.host.stored = list;
    }
    fn export(&mut self, id: &str) -> String {
        text(&self.ok("workflows.export", json!({"id": id})), "yaml")
    }
}

fn text(j: &Json, key: &str) -> String {
    j.get(key)
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("no text {key} in {}", j.stringify(false)))
        .to_string()
}

fn flag(j: &Json, key: &str) -> bool {
    j.get(key).and_then(Json::as_bool).unwrap()
}

fn id_of(j: &Json) -> String {
    text(j, "id")
}

fn list<'a>(j: &'a Json, key: &str) -> &'a Vec<Json> {
    j.get(key)
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("no list {key} in {}", j.stringify(false)))
}

fn messages(j: &Json) -> Vec<String> {
    list(j, "issues")
        .iter()
        .map(|issue| text(issue, "message"))
        .collect()
}

fn joined(j: &Json) -> String {
    messages(j).join(" ")
}

fn workflow_stub(id: &str) -> Workflow {
    Workflow::new(id, "stub")
}

// ---- workflows.add ----------------------------------------------------------------------

#[test]
fn add_rejects_unknown_fields() {
    let mut f = Fx::new();
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"extra": 1}))}),
    );
    assert!(e.contains("Unknown workflow fields: extra"), "{e}");
    assert!(
        e.contains("Allowed: name, description, inputs, requires, decisionModel, summaryTemplate, reportTo, inputValues, nodes, edges, workspace, concurrency;"),
        "{e}"
    );
    for field in [
        "id",
        "createdAt",
        "runRequested",
        "stopRequested",
        "resumeRunId",
        "nextRunAt",
        "lastRunId",
        "needsReview",
    ] {
        let e = f.err(
            "workflows.add",
            json!({"workflow": definition(json!({ field: "x" }))}),
        );
        assert!(
            e.contains(&format!("Unknown workflow fields: {field}")),
            "{field}: {e}"
        );
    }
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({})), "bogus": 1}),
    );
    assert!(e.contains("Unknown workflows.add fields: bogus"), "{e}");
    assert!(f.stored().is_empty());
}

#[test]
fn add_saves_a_clean_json_workflow_paused() {
    let mut f = Fx::new();
    let result = f.add(definition(json!({})));
    assert!(!flag(&result, "enabled"));
    assert!(list(&result, "issues").is_empty());
    assert!(text(&result, "note").to_lowercase().contains("paused"));
    let id = id_of(&result);
    assert!(
        id.len() == 11
            && id.starts_with("wf-")
            && id[3..]
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "{id}"
    );
    let saved = &f.stored()[0];
    assert_eq!(saved.id, id);
    assert_eq!(saved.name, "Ticket review");
    assert!(!saved.enabled);
    assert_eq!(saved.created_at, now());
    assert_eq!(
        saved.input_values.get("project"),
        Some(&InputValue::String("/code/app".into()))
    );
    assert_eq!(saved.next_run_at, None);
    let NodeKind::Schedule { schedule } = &saved.nodes[0].kind else {
        panic!("not a schedule")
    };
    assert_eq!(
        (schedule.schedule_kind.as_str(), schedule.time.as_str()),
        ("weekdays", "09:00")
    );
    assert_eq!(validate_workflow(saved), []);
}

#[test]
fn add_rejects_enabled_in_a_workflow_object() {
    let mut f = Fx::new();
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"enabled": true}))}),
    );
    assert!(e.contains("Unknown workflow fields: enabled"), "{e}");
    assert!(f.stored().is_empty());
}

#[test]
fn add_with_a_yaml_file_that_says_enabled_true_still_saves_it_paused() {
    let mut source = Fx::new();
    let id = id_of(&source.add(definition(json!({}))));
    let yaml = source.export(&id);
    let mut f = Fx::new();
    let result = f.ok(
        "workflows.add",
        json!({"yaml": format!("{}\nenabled: true\n", yaml.trim_end()), "values": {"project": "/code/app"}}),
    );
    assert!(!flag(&result, "enabled"));
    assert_eq!(f.stored().len(), 1);
    assert!(!f.stored()[0].enabled);
    assert_eq!(f.stored()[0].next_run_at, None);
}

#[test]
fn add_with_issues_saves_paused_and_returns_issues() {
    let mut f = Fx::new();
    let broken = definition(
        json!({"edges": [{"from": "start", "to": "fetch"}, {"from": "fetch", "to": "ghost"}]}),
    );
    let result = f.add(broken);
    assert!(!flag(&result, "enabled"));
    assert!(joined(&result).contains("ghost"), "{}", joined(&result));
    // With problems the next step is to fix them, not to enable the workflow.
    let note = text(&result, "note").to_lowercase();
    assert!(note.contains("fix"));
    assert!(!note.contains("enable"));
    assert_eq!(f.stored()[0].id, id_of(&result));
    assert!(!f.stored()[0].enabled);
    assert_eq!(f.stored()[0].next_run_at, None);
}

#[test]
fn add_with_a_model_that_is_not_in_the_catalog_saves_paused_with_the_issue() {
    let mut f = Fx::new();
    let nodes: Vec<Value> = definition_nodes()
        .into_iter()
        .map(|mut n| {
            if n["type"] == "session" {
                n["model"] = json!("sonnet");
            }
            n
        })
        .collect();
    let result = f.add(definition(json!({"nodes": nodes})));
    assert!(!flag(&result, "enabled"));
    assert_eq!(
        result.get("issues"),
        Json::parse(
            r#"[{"nodeId":"fetch","message":"Model \"sonnet\" is not available. Pick one in the step's model list."}]"#
        )
        .as_ref()
    );
    assert!(!f.stored()[0].enabled);
    assert_eq!(f.stored()[0].next_run_at, None);
}

#[test]
fn run_and_resume_refuse_a_workflow_whose_session_model_is_not_available() {
    let mut f = Fx::new();
    let nodes: Vec<Value> = definition_nodes()
        .into_iter()
        .map(|mut n| {
            if n["type"] == "session" {
                n["model"] = json!("claude-sonnet-5-5");
            }
            n
        })
        .collect();
    let id = id_of(&f.add(definition(json!({"nodes": nodes}))));
    assert!(f
        .err("workflows.run", json!({"id": id}))
        .contains("cannot run yet"));
    assert!(f
        .err("workflows.resume", json!({"id": id}))
        .contains("cannot be resumed"));
    assert!(!f.stored()[0].run_requested);
    assert_eq!(f.host.checks, 0);
}

#[test]
fn pauses_a_workflow_whose_input_has_no_value_and_says_which() {
    let mut f = Fx::new();
    let result = f.add(without(definition(json!({})), "inputValues"));
    assert!(!flag(&result, "enabled"));
    assert_eq!(
        messages(&result)[0],
        "Missing a value for input \"project\""
    );
}

#[test]
fn explains_a_node_it_cannot_read_and_enforces_limits() {
    let mut f = Fx::new();
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"nodes": [{"id": "x", "type": "teleport"}]}))}),
    );
    assert!(e.contains("nodes[0] (\"x\")"), "{e}");
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"name": "n".repeat(81)}))}),
    );
    assert!(
        e.contains("name must be a non-empty string under 80"),
        "{e}"
    );
    let e = f.err("workflows.add", json!({}));
    assert!(e.contains("either workflow"), "{e}");
    f.seed(
        (0..WORKFLOWS_MAX)
            .map(|i| workflow_stub(&format!("wf-{i}")))
            .collect(),
    );
    let e = f.err("workflows.add", json!({"workflow": definition(json!({}))}));
    assert!(e.contains("At most 100 workflows"), "{e}");
}

#[test]
fn saves_a_yaml_file_paused_with_the_commands_to_review_and_keeps_the_values_it_is_given() {
    let mut source = Fx::new();
    let id = id_of(&source.add(definition(json!({}))));
    let yaml = source.export(&id);
    let mut other = Fx::new();
    let result = other.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/other"}}),
    );
    assert!(!flag(&result, "enabled"));
    assert!(list(&result, "issues").is_empty());
    assert!(list(&result, "commands").is_empty());
    assert_eq!(text(&list(&result, "prompts")[0], "nodeId"), "fetch");
    let saved = &other.stored()[0];
    assert!(!saved.enabled);
    assert_eq!(
        saved.input_values.get("project"),
        Some(&InputValue::String("/code/other".into()))
    );
    assert_eq!(saved.next_run_at, None);
}

#[test]
fn drops_undeclared_values_from_a_yaml_add_like_an_import_does() {
    let mut source = Fx::new();
    let id = id_of(&source.add(definition(json!({}))));
    let yaml = source.export(&id);
    let mut added = Fx::new();
    let result = added.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app", "stray": "x"}}),
    );
    assert!(list(&result, "issues").is_empty());
    let kept = added.stored()[0].input_values.clone();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        kept.get("project"),
        Some(&InputValue::String("/code/app".into()))
    );
    let mut imported = Fx::new();
    imported.ok(
        "workflows.import",
        json!({"yaml": yaml, "values": {"project": "/code/app", "stray": "x"}}),
    );
    assert_eq!(imported.stored()[0].input_values, kept);
}

#[test]
fn anchors_an_interval_schedule_the_same_way_from_yaml_and_from_json() {
    let interval = json!({"kind": "interval", "hours": 4, "time": "09:00"});
    let nodes: Vec<Value> = definition_nodes()
        .into_iter()
        .map(|mut n| {
            if n["type"] == "schedule" {
                n["schedule"] = interval.clone();
            }
            n
        })
        .collect();
    let mut json_side = Fx::new();
    let id = id_of(&json_side.add(definition(json!({"nodes": nodes}))));
    let yaml = json_side.export(&id);
    let mut from_yaml = Fx::new();
    from_yaml.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    );
    for store in [&json_side, &from_yaml] {
        let NodeKind::Schedule { schedule } = &store.stored()[0].nodes[0].kind else {
            panic!("not a schedule")
        };
        assert_eq!(schedule.schedule_kind, "interval");
        assert_eq!(schedule.interval_hours, Some(4));
        assert_eq!(schedule.interval_from.as_deref(), Some("2026-10-22"));
    }
}

#[test]
fn turns_bad_mistyped_and_undeclared_input_values_into_issues_and_saves_paused() {
    let mut f = Fx::new();
    let cases = [
        (json!({"project": ""}), "needs a project"),
        (json!({"project": 7}), "needs a project"),
        (
            json!({"project": "/code/app", "stray": "x"}),
            "Unknown input \"stray\"",
        ),
    ];
    for (input_values, message) in cases {
        let result = f.add(definition(json!({"inputValues": input_values})));
        assert!(!flag(&result, "enabled"));
        assert!(joined(&result).contains(message), "{}", joined(&result));
    }
    assert!(f
        .stored()
        .iter()
        .all(|w| !w.enabled && w.next_run_at.is_none()));
}

// ---- workflows.update -------------------------------------------------------------------

#[test]
fn replaces_the_definition_but_keeps_id_created_at_input_values_enabled_and_run_state() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    // The CLI saves paused; resuming is what enables it.
    f.ok("workflows.resume", json!({"id": id}));
    let added = f.stored()[0].clone();
    let mut seeded = added.clone();
    seeded.run_requested = true;
    seeded.stop_requested = true;
    seeded.resume_run_id = Some("run-1".into());
    seeded.last_run_id = Some("run-0".into());
    seeded.imported_from = Some("somewhere".into());
    f.seed(vec![seeded]);
    let edited = without(definition(json!({"name": "Renamed"})), "inputValues");
    let result = f.ok("workflows.update", json!({"id": id, "workflow": edited}));
    assert_eq!(
        result,
        Json::parse(&json!({"id": id, "enabled": true, "issues": []}).to_string()).unwrap()
    );
    let saved = &f.stored()[0];
    assert_eq!(saved.id, id);
    assert_eq!(saved.name, "Renamed");
    assert_eq!(saved.created_at, added.created_at);
    assert_eq!(
        saved.input_values.get("project"),
        Some(&InputValue::String("/code/app".into()))
    );
    assert!(saved.enabled);
    assert_eq!(saved.next_run_at, added.next_run_at);
    assert!(saved.run_requested && saved.stop_requested);
    assert_eq!(saved.resume_run_id.as_deref(), Some("run-1"));
    assert_eq!(saved.last_run_id.as_deref(), Some("run-0"));
    assert_eq!(saved.imported_from.as_deref(), Some("somewhere"));
}

#[test]
fn drops_a_description_the_new_definition_leaves_out_and_takes_input_values_and_enabled_when_given()
{
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({"description": "old"}))));
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"inputValues": {"project": "/code/new"}, "enabled": false}))}),
    );
    let saved = &f.stored()[0];
    assert_eq!(saved.description, None);
    assert_eq!(
        saved.input_values.get("project"),
        Some(&InputValue::String("/code/new".into()))
    );
    assert!(!saved.enabled);
}

#[test]
fn pauses_a_workflow_that_no_longer_validates_and_returns_the_issues() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let result = f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"edges": [{"from": "start", "to": "ghost"}]}))}),
    );
    assert!(!flag(&result, "enabled"));
    assert!(!list(&result, "issues").is_empty());
    assert!(!f.stored()[0].enabled);
}

#[test]
fn pauses_an_update_whose_input_values_are_bad_mistyped_or_undeclared() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    for input_values in [
        json!({"project": ""}),
        json!({"project": 7}),
        json!({"project": "/p", "stray": 1}),
    ] {
        f.ok(
            "workflows.update",
            json!({"id": id, "workflow": definition(json!({"inputValues": {"project": "/code/app"}, "enabled": true}))}),
        );
        assert!(f.stored()[0].enabled);
        let result = f.ok(
            "workflows.update",
            json!({"id": id, "workflow": definition(json!({"inputValues": input_values}))}),
        );
        assert!(!flag(&result, "enabled"));
        assert!(!list(&result, "issues").is_empty());
        assert!(!f.stored()[0].enabled);
    }
}

#[test]
fn carries_over_only_the_values_of_inputs_the_new_definition_still_declares() {
    let mut f = Fx::new();
    // No inputValues in the update, so the stored ones are what is carried over.
    let without_values = |extra: Value| without(definition(extra), "inputValues");
    let id = id_of(&f.add(definition(json!({
        "inputs": {"project": {"type": "project"}, "label": {"type": "string"}},
        "inputValues": {"project": "/code/app", "label": "x"}
    }))));
    // The label input is removed; its value goes too and the workflow stays enabled.
    let result = f.ok(
        "workflows.update",
        json!({"id": id, "workflow": without_values(json!({"enabled": true}))}),
    );
    assert_eq!(
        result,
        Json::parse(&json!({"id": id, "enabled": true, "issues": []}).to_string()).unwrap()
    );
    let kept = f.stored()[0].input_values.clone();
    assert_eq!(kept.len(), 1);
    assert_eq!(
        kept.get("project"),
        Some(&InputValue::String("/code/app".into()))
    );
    // A renamed input has no value yet, which is reported, not an unknown key.
    let renamed = f.ok(
        "workflows.update",
        json!({"id": id, "workflow": without_values(json!({"inputs": {"repo": {"type": "project"}}, "enabled": true}))}),
    );
    assert!(!flag(&renamed, "enabled"));
    assert!(messages(&renamed).contains(&"Missing a value for input \"repo\"".to_string()));
    assert!(f.stored()[0].input_values.is_empty());
    // Values given explicitly are taken as given: an undeclared key is still an issue.
    let explicit = f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"inputValues": {"project": "/p", "stray": 1}}))}),
    );
    assert!(messages(&explicit)
        .contains(&"Unknown input \"stray\"; the workflow declares no such input".to_string()));
}

#[test]
fn refuses_an_unknown_id_and_runtime_fields() {
    let mut f = Fx::new();
    let e = f.err(
        "workflows.update",
        json!({"id": "wf-nope", "workflow": definition(json!({}))}),
    );
    assert!(e.contains("No workflow with that id"), "{e}");
    let id = id_of(&f.add(definition(json!({}))));
    let e = f.err(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"runRequested": true}))}),
    );
    assert!(e.contains("Unknown workflow fields: runRequested"), "{e}");
}

// ---- export and import ------------------------------------------------------------------

#[test]
fn export_then_import_round_trips_and_imports_paused() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let yaml = f.export(&id);
    assert!(yaml.contains("monoptahWorkflow: 1"));
    assert!(!yaml.contains("/code/app"));
    let imported = f.ok(
        "workflows.import",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    );
    assert!(!flag(&imported, "enabled"));
    assert!(list(&imported, "issues").is_empty());
    assert_ne!(id_of(&imported), id);
    let (original, copy) = (&f.stored()[0], &f.stored()[1]);
    assert_eq!(copy.id, id_of(&imported));
    assert!(!copy.enabled);
    assert_eq!(
        copy.input_values.get("project"),
        Some(&InputValue::String("/code/app".into()))
    );
    assert_eq!(copy.name, original.name);
    assert_eq!(copy.inputs, original.inputs);
    assert_eq!(copy.nodes, original.nodes);
    assert_eq!(copy.edges, original.edges);
}

#[test]
fn import_returns_the_same_issues_as_add_and_the_prompts_to_review() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let yaml = f.export(&id);
    let imported = f.ok(
        "workflows.import",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    );
    let got = f.ok("workflows.get", json!({"id": id_of(&imported)}));
    assert_eq!(imported.get("issues"), got.get("issues"));
    let prompts = list(&imported, "prompts");
    assert_eq!(prompts.len(), 1);
    assert_eq!(text(&prompts[0], "nodeId"), "fetch");
    assert!(text(&prompts[0], "text").contains("open tickets"));
}

#[test]
fn names_the_input_that_has_no_value_when_importing() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let yaml = f.export(&id);
    let e = f.err("workflows.import", json!({"yaml": yaml}));
    assert!(e.contains("\"project\""), "{e}");
    let e = f.err("workflows.import", json!({"yaml": "nope: 1"}));
    assert!(
        e.contains("Not a zptah workflow") || e.contains("Unsupported"),
        "{e}"
    );
    assert_eq!(f.stored().len(), 1);
}

#[test]
fn refuses_to_export_a_workflow_with_an_absolute_path_in_it() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(
        json!({"description": "Work in /Users/me/secret"}),
    )));
    let e = f.err("workflows.export", json!({"id": id}));
    assert!(e.contains("absolute path"), "{e}");
    assert!(e.starts_with("This workflow cannot be exported: "), "{e}");
}

// ---- running ----------------------------------------------------------------------------

#[test]
fn run_sets_run_requested_and_calls_check_now() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.ok("workflows.resume", json!({"id": id}));
    let result = f.ok("workflows.run", json!({"id": id}));
    assert_eq!(text(&result, "note"), "It starts now, on its own.");
    assert!(f.stored()[0].run_requested);
    assert_eq!(f.host.checks, 1);
}

#[test]
fn run_refuses_a_workflow_that_cannot_run() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.ok("workflows.resume", json!({"id": id}));
    // A run going does not refuse another one: a free slot starts it.
    f.host.live.insert(id.clone());
    let result = f.ok("workflows.run", json!({"id": id}));
    assert!(!flag(&result, "queued"));
    f.host.live.clear();
    f.host.checks = 0;
    for w in f.host.stored.iter_mut() {
        w.run_requested = false;
    }
    let broken = id_of(&f.add(definition(
        json!({"edges": [{"from": "start", "to": "ghost"}]}),
    )));
    let e = f.err("workflows.run", json!({"id": broken}));
    assert!(e.contains("cannot run yet"), "{e}");
    assert!(!f.stored().iter().any(|w| w.run_requested));
    assert_eq!(f.host.checks, 0);
}

#[test]
fn run_refuses_a_paused_workflow_and_says_to_resume_it_first() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.ok("workflows.pause", json!({"id": id}));
    let e = f.err("workflows.run", json!({"id": id}));
    assert_eq!(e, "Resume the workflow before running it.");
    assert!(!f.stored()[0].run_requested);
    assert_eq!(f.host.checks, 0);
}

#[test]
fn list_reports_running_state() {
    let mut f = Fx::new();
    let first = id_of(&f.add(definition(json!({}))));
    let second = id_of(&f.add(definition(json!({"name": "Other"}))));
    f.ok("workflows.resume", json!({"id": first}));
    f.host.live.insert(first.clone());
    f.host.runs.insert(
        first.clone(),
        vec![run("run-1", RunStatus::Running, &first)],
    );
    let result = f.ok("workflows.list", json!({}));
    let workflows = list(&result, "workflows");
    assert_eq!(workflows.len(), 2);
    let (a, b) = (&workflows[0], &workflows[1]);
    assert_eq!(text(a, "id"), first);
    assert_eq!(text(a, "name"), "Ticket review");
    assert!(flag(a, "enabled"));
    let triggers = list(a, "triggers");
    assert_eq!(triggers.len(), 1);
    assert_eq!(text(&triggers[0], "node"), "start");
    assert_eq!(text(&triggers[0], "type"), "schedule");
    assert_eq!(text(&triggers[0], "schedule"), "Weekdays at 09:00");
    assert!(flag(a, "running"));
    let last = a.get("lastRun").unwrap();
    assert_eq!(
        (text(last, "id"), text(last, "status")),
        ("run-1".into(), "running".into())
    );
    assert_eq!(text(b, "id"), second);
    assert!(!flag(b, "running"));
    assert_eq!(b.get("lastRun"), Some(&Json::Null));
    // A paused workflow has no next run; a resumed one reads in local time.
    assert_eq!(b.get("nextRunAt"), Some(&Json::Null));
    let expected = Local
        .timestamp_millis_opt(f.stored()[0].next_run_at.unwrap() as i64)
        .unwrap()
        .format("%Y-%m-%d %H:%M")
        .to_string();
    assert_eq!(text(a, "nextRunAt"), expected);
    assert_eq!(
        text(last, "startedAt"),
        Local
            .timestamp_millis_opt((now() - 60_000) as i64)
            .unwrap()
            .format("%Y-%m-%d %H:%M")
            .to_string()
    );
}

#[test]
fn stop_goes_through_the_files_so_any_window_can_stop_and_only_a_running_workflow() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let e = f.err("workflows.stop", json!({"id": id}));
    assert!(e.contains("not running"), "{e}");
    f.host.live.insert(id.clone());
    let result = f.ok("workflows.stop", json!({"id": id}));
    assert_eq!(
        text(&result, "note"),
        "Stop requested; the run ends within seconds."
    );
    assert!(f.stored()[0].stop_requested);
    assert_eq!(f.host.checks, 1);
}

#[test]
fn resume_run_only_resumes_an_interrupted_run() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.host.runs.insert(
        id.clone(),
        vec![
            run("run-2", RunStatus::Done, &id),
            run("run-1", RunStatus::Interrupted, &id),
        ],
    );
    let e = f.err("workflows.resumeRun", json!({"id": id, "runId": "nope"}));
    assert!(e.contains("No run"), "{e}");
    let e = f.err("workflows.resumeRun", json!({"id": id, "runId": "run-2"}));
    assert_eq!(
        e,
        "Only an interrupted run can be resumed; this one is done"
    );
    assert_eq!(f.stored()[0].resume_run_id, None);
    let result = f.ok("workflows.resumeRun", json!({"id": id, "runId": "run-1"}));
    assert_eq!(text(&result, "note"), "It continues from where it stopped.");
    assert_eq!(f.stored()[0].resume_run_id.as_deref(), Some("run-1"));
    assert_eq!(f.host.checks, 1);
}

// ---- pause, resume, setInputs, remove, get and runs -------------------------------------

#[test]
fn pauses_and_resumes_computing_the_next_time_on_resume() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.ok("workflows.pause", json!({"id": id}));
    assert!(!f.stored()[0].enabled);
    f.ok("workflows.resume", json!({"id": id}));
    assert!(f.stored()[0].enabled);
    assert!(f.stored()[0].next_run_at.unwrap() > now());
}

#[test]
fn will_not_resume_a_workflow_with_issues() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(
        json!({"edges": [{"from": "start", "to": "ghost"}]}),
    )));
    let e = f.err("workflows.resume", json!({"id": id}));
    assert!(e.contains("cannot be resumed"), "{e}");
}

#[test]
fn sets_declared_inputs_with_their_type_and_rejects_others() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({
        "inputs": {"project": {"type": "project"}, "count": {"type": "number"}}
    }))));
    assert!(!f.stored()[0].enabled);
    let result = f.ok(
        "workflows.setInputs",
        json!({"id": id, "values": {"count": 3}}),
    );
    assert_eq!(
        result.get("inputValues"),
        Json::parse(r#"{"project":"/code/app","count":3}"#).as_ref()
    );
    assert_eq!(list(&result, "issues").len(), 0);
    let bad = |f: &mut Fx, values: Value| {
        f.err("workflows.setInputs", json!({"id": id, "values": values}))
    };
    assert!(bad(&mut f, json!({"count": "x"})).contains("must be a number"));
    assert!(bad(&mut f, json!({"nope": 1})).contains("Unknown input \"nope\""));
    assert!(bad(&mut f, json!({"project": ""})).contains("needs a project"));
    assert!(bad(&mut f, json!({"project": 7})).contains("needs a project"));
    assert!(bad(&mut f, json!({"toString": "x"})).contains("Unknown input \"toString\""));
    let declared = bad(&mut f, json!({"nope": 1}));
    assert!(declared.contains(". Declared inputs: "), "{declared}");
    assert!(
        declared.contains("project") && declared.contains("count"),
        "{declared}"
    );
    let values = &f.stored()[0].input_values;
    assert_eq!(values.len(), 2);
    assert_eq!(values.get("count"), Some(&InputValue::Number(3.0)));
    assert_eq!(
        values.get("project"),
        Some(&InputValue::String("/code/app".into()))
    );
}

#[test]
fn removes_a_workflow_unless_it_is_running() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.host.live.insert(id.clone());
    let e = f.err("workflows.remove", json!({"id": id}));
    assert!(e.contains("Stop the workflow"), "{e}");
    f.host.live.clear();
    let removed = f.ok("workflows.remove", json!({"id": id}));
    assert_eq!(
        removed,
        Json::parse(r#"{"removed":"Ticket review"}"#).unwrap()
    );
    assert!(f.stored().is_empty());
}

#[test]
fn get_returns_the_definition_and_the_last_five_runs_runs_shows_branches() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.host.runs.insert(
        id.clone(),
        (0..7)
            .map(|i| run(&format!("run-{i}"), RunStatus::Done, &id))
            .collect(),
    );
    let got = f.ok("workflows.get", json!({"id": id}));
    assert_eq!(list(&got, "nodes").len(), 2);
    assert_eq!(list(&got, "runs").len(), 5);
    assert_eq!(list(&got, "issues").len(), 0);
    assert_eq!(
        got.get("inputValues"),
        Json::parse(r#"{"project":"/code/app"}"#).as_ref()
    );
    let summaries = f.ok("workflows.runs", json!({"id": id}));
    let summaries = list(&summaries, "runs");
    assert_eq!(summaries.len(), 5);
    assert_eq!(
        list(&summaries[0], "nodes")[1],
        Json::parse(
            r#"{"nodeId":"dev","branch":"each#0","status":"failed","round":1,"error":"boom"}"#
        )
        .unwrap()
    );
    assert_eq!(
        list(
            &f.ok("workflows.runs", json!({"id": id, "limit": 2})),
            "runs"
        )
        .len(),
        2
    );
    let e = f.err("workflows.runs", json!({"id": id, "limit": 0}));
    assert!(e.contains("limit"), "{e}");
}

// ---- a workflow the CLI created from yaml needs the user's review -----------------------

const REVIEW: [&str; 2] = ["came from a file", "enable it in Workflows"];

fn assert_review(e: &str) {
    let at = e.find(REVIEW[0]).unwrap_or_else(|| panic!("{e}"));
    assert!(e[at..].contains(REVIEW[1]), "{e}");
}

/// A host whose history holds nothing, and the yaml of a workflow it exported then removed.
fn imported() -> (Fx, String) {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let yaml = f.export(&id);
    f.ok("workflows.remove", json!({"id": id}));
    (f, yaml)
}

#[test]
fn marks_workflows_add_with_yaml_and_workflows_import_but_not_a_definition_written_here() {
    let (mut f, yaml) = imported();
    f.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    );
    f.ok(
        "workflows.import",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    );
    f.add(definition(json!({"name": "Written here"})));
    let marks: Vec<(&str, bool)> = f
        .stored()
        .iter()
        .map(|w| (w.name.as_str(), w.needs_review))
        .collect();
    assert_eq!(
        marks,
        [
            ("Ticket review", true),
            ("Ticket review", true),
            ("Written here", false)
        ]
    );
}

#[test]
fn refuses_resume_run_and_update_with_enabled_true() {
    let (mut f, yaml) = imported();
    let id = id_of(&f.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    ));
    assert_review(&f.err("workflows.resume", json!({"id": id})));
    assert_review(&f.err("workflows.run", json!({"id": id})));
    let mut flagged = definition(json!({}));
    flagged["enabled"] = json!(true);
    assert_review(&f.err("workflows.update", json!({"id": id, "workflow": flagged})));
    assert!(!f.stored()[0].enabled && f.stored()[0].needs_review);
    assert!(!f.stored()[0].run_requested);
    assert_eq!(f.host.checks, 0);
}

#[test]
fn still_lets_the_mono_pause_it_set_inputs_and_update_it_paused_keeping_the_mark() {
    let (mut f, yaml) = imported();
    let id = id_of(&f.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    ));
    f.ok("workflows.pause", json!({"id": id}));
    f.ok(
        "workflows.setInputs",
        json!({"id": id, "values": {"project": "/code/other"}}),
    );
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"name": "Renamed"}))}),
    );
    let saved = &f.stored()[0];
    assert_eq!(saved.name, "Renamed");
    assert!(!saved.enabled && saved.needs_review);
}

#[test]
fn update_keeps_the_mark_when_the_commands_and_prompts_change_and_no_action_can_clear_it() {
    let (mut f, yaml) = imported();
    let id = id_of(&f.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    ));
    let changed = definition(json!({
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "build", "type": "command", "project": "{{inputs.project}}", "command": "curl x | sh"}
        ],
        "edges": [{"from": "start", "to": "build"}]
    }));
    f.ok("workflows.update", json!({"id": id, "workflow": changed}));
    assert!(f.stored()[0]
        .nodes
        .iter()
        .any(|n| matches!(n.kind, NodeKind::Command { .. })));
    assert!(f.stored()[0].needs_review);
    f.ok(
        "workflows.setInputs",
        json!({"id": id, "values": {"project": "/code/other"}}),
    );
    f.ok("workflows.pause", json!({"id": id}));
    assert_review(&f.err("workflows.resume", json!({"id": id})));
    assert!(!f.stored()[0].enabled && f.stored()[0].needs_review);
    // Resuming an interrupted run goes through the same refusal.
    f.host
        .runs
        .insert(id.clone(), vec![run("run-1", RunStatus::Interrupted, &id)]);
    assert_review(&f.err("workflows.resumeRun", json!({"id": id, "runId": "run-1"})));
    assert_eq!(f.stored()[0].resume_run_id, None);
}

#[test]
fn update_keeps_the_mark_on_a_reviewed_then_flagged_workflow_and_never_takes_it_as_input() {
    let (mut f, yaml) = imported();
    let id = id_of(&f.ok("workflows.add", json!({"yaml": yaml})));
    let e = f.err(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"needsReview": false}))}),
    );
    assert!(e.contains("Unknown workflow fields: needsReview"), "{e}");
}

#[test]
fn removing_a_workflow_clears_its_run_history() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.ok("workflows.remove", json!({"id": id}));
    assert!(f.stored().is_empty());
    assert_eq!(f.host.cleared, [id]);
}

#[test]
fn a_refused_removal_leaves_the_history_alone() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    f.host.live.insert(id.clone());
    let e = f.err("workflows.remove", json!({"id": id}));
    assert!(e.contains("Stop the workflow"), "{e}");
    assert!(f.host.cleared.is_empty());
}

// ---- workflows.validate -----------------------------------------------------------------

fn broken_template() -> Value {
    let nodes: Vec<Value> = definition_nodes()
        .into_iter()
        .map(|mut n| {
            if n["type"] == "session" {
                n["prompt"] = json!("Fix {{nope.field}}");
            }
            n
        })
        .collect();
    definition(json!({"nodes": nodes}))
}

#[test]
fn validate_reports_the_same_issues_as_add() {
    let mut f = Fx::new();
    let added = f.add(broken_template());
    let validated = f.ok("workflows.validate", json!({"workflow": broken_template()}));
    assert!(!list(&added, "issues").is_empty());
    assert_eq!(validated.get("issues"), added.get("issues"));
}

#[test]
fn validate_reports_a_missing_input_value_and_none_when_values_complete_it() {
    let mut f = Fx::new();
    let bare = without(definition(json!({})), "inputValues");
    let missing = f.ok("workflows.validate", json!({"workflow": bare}));
    assert_eq!(
        messages(&missing)[0],
        "Missing a value for input \"project\""
    );
    let mut source = Fx::new();
    let id = id_of(&source.add(bare));
    let yaml = source.export(&id);
    let with_values = f.ok(
        "workflows.validate",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    );
    assert!(list(&with_values, "issues").is_empty());
}

#[test]
fn validate_judges_an_existing_workflows_input_values_the_way_update_would() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let bare = without(definition(json!({"name": "Renamed"})), "inputValues");
    let before = f.stored().to_vec();
    // The stored project value stays when the change leaves inputValues out, as update would keep it.
    let kept = f.ok("workflows.validate", json!({"id": id, "workflow": bare}));
    assert!(list(&kept, "issues").is_empty());
    // Without an id there is nothing stored to keep.
    let fresh = f.ok("workflows.validate", json!({"workflow": bare}));
    assert_eq!(messages(&fresh)[0], "Missing a value for input \"project\"");
    // Given values win over the stored ones.
    let mut given_def = bare.clone();
    given_def["inputValues"] = json!({"project": ""});
    let given = f.ok(
        "workflows.validate",
        json!({"id": id, "workflow": given_def}),
    );
    assert!(!list(&given, "issues").is_empty());
    assert_eq!(f.stored(), before);
}

#[test]
fn validate_never_writes_for_valid_and_invalid_input() {
    let mut f = Fx::new();
    f.ok(
        "workflows.validate",
        json!({"workflow": definition(json!({}))}),
    );
    f.ok("workflows.validate", json!({"workflow": broken_template()}));
    f.err(
        "workflows.validate",
        json!({"workflow": definition(json!({"name": ""}))}),
    );
    assert!(f.stored().is_empty());
    assert_eq!(f.host.updates, 0);
}

#[test]
fn validate_lists_commands_and_prompts_verbatim() {
    let mut f = Fx::new();
    let mut nodes: Vec<Value> = definition_nodes()
        .into_iter()
        .filter(|n| n["type"] != "report")
        .collect();
    nodes.push(json!({"id": "build", "type": "command", "project": "{{inputs.project}}", "command": "npm test -- --run"}));
    let result = f.ok(
        "workflows.validate",
        json!({"workflow": definition(json!({
            "nodes": nodes,
            "edges": [
                {"from": "start", "to": "fetch"},
                {"from": "fetch", "to": "build"}
            ]
        }))}),
    );
    assert_eq!(
        result.get("commands"),
        Json::parse(r#"[{"nodeId":"build","command":"npm test -- --run"}]"#).as_ref()
    );
    // The prompt is shown as the step sends it: the written text, then the hint about the
    // declared outputs and the command that records them.
    let prompts = list(&result, "prompts");
    assert_eq!(prompts.len(), 1);
    assert_eq!(text(&prompts[0], "nodeId"), "fetch");
    assert!(text(&prompts[0], "text").starts_with("List my open tickets."));
    assert!(text(&prompts[0], "text").contains("Run: zptah app workflow.output '"));
    assert!(!text(&prompts[0], "text").contains("app app"));
}

#[test]
fn validate_accepts_the_same_input_shapes_as_add() {
    let mut f = Fx::new();
    assert!(f
        .err("workflows.validate", json!({}))
        .contains("either workflow"));
    assert!(f
        .err(
            "workflows.validate",
            json!({"workflow": definition(json!({})), "yaml": "x"})
        )
        .contains("either workflow"));
    assert!(f
        .err(
            "workflows.validate",
            json!({"workflow": definition(json!({})), "values": {"project": "x"}})
        )
        .contains("values goes with yaml"));
}

// ---- workflows.validate against the per-Mono limit --------------------------------------

fn seed_full(f: &mut Fx, count: usize) {
    f.seed(
        (0..count)
            .map(|i| {
                let mut w = Workflow::new(format!("wf-{i:08}"), format!("Workflow {i}"));
                w.nodes = vec![Node::new("start", NodeKind::Manual)];
                w.created_at = 1;
                w
            })
            .collect(),
    );
}

#[test]
fn validate_reports_an_issue_when_the_mono_is_at_the_limit_and_still_writes_nothing() {
    let mut f = Fx::new();
    seed_full(&mut f, WORKFLOWS_MAX);
    let result = f.ok(
        "workflows.validate",
        json!({"workflow": definition(json!({}))}),
    );
    assert!(
        joined(&result).contains(&format!("At most {WORKFLOWS_MAX} workflows")),
        "{}",
        joined(&result)
    );
    assert_eq!(f.stored().len(), WORKFLOWS_MAX);
    assert_eq!(f.host.updates, 0);
    let e = f.err("workflows.add", json!({"workflow": definition(json!({}))}));
    assert!(
        e.contains(&format!("At most {WORKFLOWS_MAX} workflows")),
        "{e}"
    );
}

#[test]
fn validate_does_not_apply_the_limit_when_id_names_an_existing_workflow() {
    let mut f = Fx::new();
    seed_full(&mut f, WORKFLOWS_MAX);
    let first = f.stored()[0].id.clone();
    let result = f.ok(
        "workflows.validate",
        json!({"id": first, "workflow": definition(json!({}))}),
    );
    assert!(list(&result, "issues").is_empty());
    // An id that names nothing is a new workflow, so the limit still applies.
    let unknown = f.ok(
        "workflows.validate",
        json!({"id": "wf-zzzzzzzz", "workflow": definition(json!({}))}),
    );
    assert!(joined(&unknown).contains(&format!("At most {WORKFLOWS_MAX} workflows")));
    assert_eq!(f.host.updates, 0);
    // The update the validation described is accepted at the limit.
    let updated = f.ok(
        "workflows.update",
        json!({"id": first, "workflow": definition(json!({}))}),
    );
    assert_eq!(id_of(&updated), first);
    assert!(list(&updated, "issues").is_empty());
}

#[test]
fn validate_reports_nothing_extra_below_the_limit() {
    let mut f = Fx::new();
    seed_full(&mut f, WORKFLOWS_MAX - 1);
    let result = f.ok(
        "workflows.validate",
        json!({"workflow": definition(json!({}))}),
    );
    assert!(list(&result, "issues").is_empty());
}

// ---- the guide --------------------------------------------------------------------------

#[test]
fn every_example_is_stored_by_workflows_add_exactly_as_written() {
    for example in guide_examples() {
        let mut f = Fx::new();
        let sent = Json::parse(&example.workflow.stringify(false)).unwrap();
        let result = handle(
            "workflows.add",
            &Json::parse(&format!(r#"{{"workflow":{}}}"#, sent.stringify(false))).unwrap(),
            &mut f.host,
        )
        .unwrap();
        assert_eq!(messages(&result), Vec::<String>::new(), "{}", example.name);
        let saved = &f.stored()[0];
        let w = &example.workflow;
        assert_eq!(
            Some(saved.name.as_str()),
            w.get("name").and_then(Json::as_str)
        );
        assert_eq!(
            saved.description.as_deref(),
            w.get("description").and_then(Json::as_str)
        );
        let stored = super::store::workflow_json(saved).unwrap();
        for key in ["inputs", "inputValues", "edges", "nodes"] {
            assert_eq!(stored.get(key), w.get(key), "{} {key}", example.name);
        }
    }
}

#[test]
fn workflows_guide_returns_the_guide() {
    let mut f = Fx::new();
    let got = f.ok("workflows.guide", json!({}));
    assert_eq!(text(&got, "guide"), workflow_guide());
}

#[test]
fn guide_takes_no_input() {
    let mut f = Fx::new();
    let e = f.err("workflows.guide", json!({"id": "x"}));
    assert!(e.contains("Unknown workflows.guide fields: id"), "{e}");
}

// ---- extras -----------------------------------------------------------------------------

#[test]
fn an_unknown_action_and_a_non_object_input_are_refused() {
    let mut f = Fx::new();
    assert_eq!(
        f.err("workflows.nope", json!({})),
        "Unknown app action: workflows.nope"
    );
    let e = handle("workflows.list", &Json::Null, &mut f.host).unwrap_err();
    assert_eq!(e, "Input must be an object");
}

#[test]
fn field_lists_cover_the_sixteen_actions() {
    let actions = [
        "list",
        "get",
        "add",
        "validate",
        "guide",
        "update",
        "setInputs",
        "run",
        "stop",
        "pause",
        "resume",
        "resumeRun",
        "remove",
        "export",
        "import",
        "runs",
    ];
    for action in actions {
        let fields = fields(&format!("workflows.{action}")).unwrap();
        assert!(
            fields.split_whitespace().any(|f| f == "project"),
            "{action}"
        );
    }
    assert_eq!(fields("workflows.other"), None);
}

#[test]
fn definition_errors_say_what_is_wrong() {
    let mut f = Fx::new();
    let mut check = |workflow: Value, message: &str| {
        let e = f.err("workflows.add", json!({"workflow": workflow}));
        assert!(e.contains(message), "{message}: {e}");
    };
    check(json!("nope"), "workflow must be an object");
    check(
        definition(json!({"description": ""})),
        "workflow.description must be a non-empty string under 1000",
    );
    check(
        definition(json!({"nodes": []})),
        "workflow.nodes must be a list of nodes",
    );
    check(
        definition(json!({"edges": {"a": 1}})),
        "workflow.edges must be a list",
    );
    check(
        definition(json!({"edges": [{"from": "a"}]})),
        "edges[0] needs \"from\" and \"to\"",
    );
    check(
        definition(json!({"inputs": "x"})),
        "workflow.inputs must be an object",
    );
    check(
        definition(json!({"inputs": {"bad name": {"type": "project"}, "ok": {"type": "weird"}}})),
        "Invalid inputs: bad name, ok",
    );
    check(
        definition(json!({"inputValues": "x"})),
        "workflow.inputValues must be an object of values",
    );
    check(
        definition(json!({"inputValues": {"project": null}})),
        "workflow.inputValues.project must be text, a number or true or false",
    );
    check(
        definition(
            json!({"nodes": [{"id": "start", "type": "schedule", "schedule": {"kind": "monthly"}}]}),
        ),
        "Node \"start\": schedule.kind must be hourly, daily, weekdays, weekly or interval",
    );
    check(
        definition(json!({"name": "x".repeat(600_000)})),
        "The workflow is too large (over 512 KiB)",
    );
}

#[test]
fn a_nameless_id_and_a_long_id_are_refused() {
    let mut f = Fx::new();
    let e = f.err("workflows.get", json!({}));
    assert_eq!(e, "id must be a non-empty string under 128 characters");
    let e = f.err("workflows.get", json!({"id": "x".repeat(129)}));
    assert!(e.starts_with("id must be a non-empty string"), "{e}");
    let e = f.err("workflows.get", json!({"id": "wf-nope"}));
    assert_eq!(e, "No workflow with that id; run workflows.list");
}

#[test]
fn runs_limit_is_a_whole_number_from_1_to_20() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    for bad in [json!(0), json!(21), json!(1.5), json!("3"), json!(true)] {
        let e = f.err("workflows.runs", json!({"id": id, "limit": bad}));
        assert_eq!(e, "limit must be a whole number from 1 to 20");
    }
    f.ok("workflows.runs", json!({"id": id, "limit": 20}));
    // `limit ?? 5`: null is as good as absent.
    f.ok("workflows.runs", json!({"id": id, "limit": null}));
}

#[test]
fn a_run_summary_shows_its_end_time_and_error() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let mut failed = run("run-1", RunStatus::Failed, &id);
    failed.ended_at = Some(now());
    failed.error = Some("it broke".into());
    f.host.runs.insert(id.clone(), vec![failed]);
    let got = f.ok("workflows.get", json!({"id": id}));
    let summary = &list(&got, "runs")[0];
    assert_eq!(text(summary, "error"), "it broke");
    assert!(summary.get("endedAt").is_some());
    assert_eq!(
        summary.get("caller").map(|c| text(c, "kind")),
        Some("manual".to_string())
    );
    assert_eq!(
        summary.get("caller").map(|c| text(c, "triggerNode")),
        Some("start".to_string())
    );
    assert!(summary.get("nodes").is_none());
}

// ---- cloud instances a run used ---------------------------------------------------------

fn used(kind: &str, name: &str, created: bool) -> RunInstance {
    RunInstance {
        kind: kind.into(),
        name: name.into(),
        created,
        nodes: vec!["build".into()],
        sessions: vec!["remote:1".into()],
    }
}

#[test]
fn get_and_runs_list_the_cloud_instances_and_warn_for_an_interrupted_run() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let mut interrupted = run("run-2", RunStatus::Interrupted, &id);
    interrupted.cloud_instances = vec![used("codespace", "octo-run", true)];
    let mut done = run("run-1", RunStatus::Done, &id);
    done.cloud_instances = vec![used("sandbox", "box", false)];
    let plain = run("run-0", RunStatus::Done, &id);
    f.host
        .runs
        .insert(id.clone(), vec![interrupted, done, plain]);

    let got = f.ok("workflows.get", json!({"id": id}));
    let runs = list(&got, "runs");
    let first = list(&runs[0], "cloudInstances");
    assert_eq!(text(&first[0], "kind"), "codespace");
    assert_eq!(text(&first[0], "name"), "octo-run");
    assert!(flag(&first[0], "createdByRun"));
    assert_eq!(list(&first[0], "steps").len(), 1);
    let warning = text(&runs[0], "cloudInstancesWarning");
    assert!(
        warning.starts_with("This run left 1 cloud instance running: Codespace octo-run."),
        "{warning}"
    );
    // A finished run lists them too (the report named them) but has nothing to warn about.
    assert_eq!(list(&runs[1], "cloudInstances").len(), 1);
    assert!(runs[1].get("cloudInstancesWarning").is_none());
    assert!(runs[2].get("cloudInstances").is_none());
    // The session ids are the app's business, not the Mono's.
    assert!(!got.stringify(false).contains("remote:1"));

    let detail = f.ok("workflows.runs", json!({"id": id}));
    assert_eq!(list(&list(&detail, "runs")[0], "cloudInstances").len(), 1);
}

#[test]
fn get_names_the_instances_of_every_interrupted_run_not_only_the_last_five() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    // Eight runs, newest first: the interrupted ones that left instances are 7th and 8th.
    let mut runs: Vec<WorkflowRun> = (0..6)
        .map(|i| run(&format!("run-{i}"), RunStatus::Done, &id))
        .collect();
    let mut old = run("run-6", RunStatus::Interrupted, &id);
    old.cloud_instances = vec![used("codespace", "octo-run", true)];
    let mut older = run("run-7", RunStatus::Interrupted, &id);
    older.cloud_instances = vec![
        used("codespace", "octo-run", false),
        used("sandbox", "box", true),
    ];
    runs.push(old);
    runs.push(older);
    f.host.runs.insert(id.clone(), runs);
    let got = f.ok("workflows.get", json!({"id": id}));
    assert_eq!(list(&got, "runs").len(), 5);
    let left = list(&got, "cloudInstancesLeft");
    assert_eq!(
        left.iter().map(|i| text(i, "name")).collect::<Vec<_>>(),
        ["octo-run", "box"]
    );
    let note = text(&got, "cloudInstancesLeftNote");
    assert!(
        note.contains("left 2 cloud instances running: Codespace octo-run, Sandbox box"),
        "{note}"
    );
    // Nothing interrupted, nothing to say.
    f.host
        .runs
        .insert(id.clone(), vec![run("run-9", RunStatus::Done, &id)]);
    assert!(f
        .ok("workflows.get", json!({"id": id}))
        .get("cloudInstancesLeft")
        .is_none());
}

#[test]
fn stop_and_remove_name_the_cloud_instances_and_touch_none() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({}))));
    let mut running = run("run-1", RunStatus::Running, &id);
    running.cloud_instances = vec![used("codespace", "octo-run", true)];
    f.host.runs.insert(id.clone(), vec![running]);
    f.host.live.insert(id.clone());
    let stopped = f.ok("workflows.stop", json!({"id": id}));
    assert_eq!(list(&stopped, "cloudInstances").len(), 1);
    assert!(
        text(&stopped, "cloudInstancesNote")
            .contains("left 1 cloud instance running: Codespace octo-run"),
        "{}",
        stopped.stringify(false)
    );
    // A run with no instance keeps the plain answer.
    f.host
        .runs
        .insert(id.clone(), vec![run("run-2", RunStatus::Running, &id)]);
    let plain = f.ok("workflows.stop", json!({"id": id}));
    assert!(plain.get("cloudInstances").is_none());

    // Removing the workflow forgets its interrupted runs: their instances are named once.
    f.host.live.clear();
    let mut first = run("run-3", RunStatus::Interrupted, &id);
    first.cloud_instances = vec![used("codespace", "octo-run", true)];
    let mut second = run("run-4", RunStatus::Interrupted, &id);
    second.cloud_instances = vec![
        used("codespace", "octo-run", false),
        used("sandbox", "box", true),
    ];
    let mut finished = run("run-5", RunStatus::Done, &id);
    finished.cloud_instances = vec![used("codespace", "reported-one", true)];
    f.host
        .runs
        .insert(id.clone(), vec![first, second, finished]);
    let removed = f.ok("workflows.remove", json!({"id": id}));
    let named = list(&removed, "cloudInstances");
    assert_eq!(
        named.iter().map(|i| text(i, "name")).collect::<Vec<_>>(),
        ["octo-run", "box"]
    );
    let note = text(&removed, "cloudInstancesNote");
    assert!(
        note.contains("left 2 cloud instances running: Codespace octo-run, Sandbox box"),
        "{note}"
    );
    assert!(note.contains("never stops or deletes"), "{note}");
    assert!(f.stored().is_empty());
    assert_eq!(f.host.cleared, [id]);
}

/// The small definition with its `fetch` step in a cloud instance as `account`.
fn account_definition(account: &str, instance: Value) -> Value {
    let mut def = definition(json!({}));
    def["nodes"][1]["target"] = json!({"kind": "codespace", "instance": instance});
    def["nodes"][1]["account"] = json!(account);
    def
}

#[test]
fn validate_add_and_update_report_an_account_the_host_cannot_use() {
    let mut f = Fx::new();
    f.host
        .bad_accounts
        .insert("Nobody".into(), "No Claude account is named Nobody.".into());
    f.host.bad_accounts.insert(
        "Bare".into(),
        "A cloud session needs a Claude account with a setup token or API key.".into(),
    );
    for (account, phrase) in [
        ("Nobody", "account: No Claude account is named Nobody."),
        ("Bare", "account: A cloud session needs a Claude account"),
    ] {
        let def = account_definition(account, json!("new"));
        let checked = f.ok("workflows.validate", json!({"workflow": def.clone()}));
        assert!(
            joined(&checked).contains(phrase),
            "{account}: {}",
            joined(&checked)
        );
        let issue = list(&checked, "issues")
            .iter()
            .find(|i| text(i, "message").starts_with("account:"))
            .unwrap();
        assert_eq!(text(issue, "nodeId"), "fetch");
        // Saved paused with the issue, like any other problem.
        let added = f.add(def.clone());
        assert!(joined(&added).contains(phrase), "{account}");
        let id = id_of(&added);
        let updated = f.ok("workflows.update", json!({"id": id, "workflow": def}));
        assert!(joined(&updated).contains(phrase), "{account}");
    }
    // A usable account, and no account, raise nothing.
    let good = f.ok(
        "workflows.validate",
        json!({"workflow": account_definition("Work", json!("new"))}),
    );
    assert!(list(&good, "issues").is_empty(), "{}", joined(&good));
    // A step that goes on in another instance takes no account: validation says so, and the
    // host is not asked about it.
    let same = f.ok(
        "workflows.validate",
        json!({"workflow": account_definition("Nobody", json!({"sameAs": "fetch"}))}),
    );
    let said = joined(&same);
    assert!(
        said.contains("The account belongs to the instance"),
        "{said}"
    );
    assert!(!said.contains("No Claude account"), "{said}");
}

// ---- the store-backed host --------------------------------------------------------------

fn no_probe(_: Kind) -> Probe {
    Probe {
        installed: false,
        models: None,
    }
}

fn store_host() -> (StoreHost, Arc<Signals>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("wf-cli-{}", crate::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    let signals = Arc::new(Signals::default());
    let host = StoreHost::new(WorkflowsDir::new(root.join("workflows")), signals.clone())
        .with_probe(no_probe);
    (host, signals, root)
}

fn manual_workflow() -> Value {
    json!({
        "name": "Manual",
        "nodes": [{"id": "start", "type": "manual"}],
        "edges": []
    })
}

#[test]
fn the_store_host_saves_to_the_mono_files_and_signals_the_runner() {
    let (mut host, signals, root) = store_host();
    let mut call = |action: &str, input: Value| handle(action, &Json::from(&input), &mut host);
    let added = call("workflows.add", json!({"workflow": manual_workflow()})).unwrap();
    let id = id_of(&added);
    let dir = WorkflowsDir::new(root.join("workflows"));
    let on_disk = load_workflows(&dir).unwrap().workflows;
    assert_eq!(on_disk.len(), 1);
    assert_eq!(on_disk[0].id, id);
    assert!(!on_disk[0].enabled);
    call("workflows.resume", json!({"id": id})).unwrap();
    assert!(!signals.take_check_now());
    call("workflows.run", json!({"id": id})).unwrap();
    assert!(load_workflows(&dir).unwrap().workflows[0].run_requested);
    assert!(signals.take_check_now());
    assert!(!signals.take_check_now(), "a check is taken once");
    // Nothing is running until the runner says so.
    let e = call("workflows.stop", json!({"id": id})).unwrap_err();
    assert!(e.contains("not running"), "{e}");
    signals.set_running(&id, true);
    call("workflows.stop", json!({"id": id})).unwrap();
    assert!(load_workflows(&dir).unwrap().workflows[0].stop_requested);
    assert!(signals.take_check_now());
    let listed = call("workflows.list", json!({})).unwrap();
    assert!(flag(&list(&listed, "workflows")[0], "running"));
    let e = call("workflows.remove", json!({"id": id})).unwrap_err();
    assert!(e.contains("Stop the workflow"), "{e}");
    signals.set_running(&id, false);
    call("workflows.remove", json!({"id": id})).unwrap();
    assert!(load_workflows(&dir).unwrap().workflows.is_empty());
    assert!(load_runs(&dir, &id).unwrap().is_empty());
}

#[test]
fn the_store_host_resumes_an_interrupted_run_through_the_file() {
    let (mut host, signals, root) = store_host();
    let dir = WorkflowsDir::new(root.join("workflows"));
    let id = id_of(
        &handle(
            "workflows.add",
            &Json::from(&json!({"workflow": manual_workflow()})),
            &mut host,
        )
        .unwrap(),
    );
    super::store::update_runs(&dir, &id, |_| {
        Ok((vec![run("run-1", RunStatus::Interrupted, &id)], ()))
    })
    .unwrap();
    let result = handle(
        "workflows.resumeRun",
        &Json::from(&json!({"id": id, "runId": "run-1"})),
        &mut host,
    )
    .unwrap();
    assert_eq!(text(&result, "runId"), "run-1");
    assert_eq!(
        load_workflows(&dir).unwrap().workflows[0]
            .resume_run_id
            .as_deref(),
        Some("run-1")
    );
    assert!(signals.take_check_now());
}

#[test]
fn a_failure_clearing_the_history_does_not_block_a_removal() {
    let (mut host, _, root) = store_host();
    let id = id_of(
        &handle(
            "workflows.add",
            &Json::from(&json!({"workflow": manual_workflow()})),
            &mut host,
        )
        .unwrap(),
    );
    // A file where the runs folder should be: clearing the history cannot write there.
    std::fs::write(root.join("workflows").join("runs"), "x").unwrap();
    let removed = handle(
        "workflows.remove",
        &Json::from(&json!({"id": id})),
        &mut host,
    )
    .unwrap();
    assert_eq!(text(&removed, "removed"), "Manual");
}

#[test]
fn the_store_host_judges_models_with_its_probe_and_never_asks_for_unused_harnesses() {
    fn claude_only(kind: Kind) -> Probe {
        assert_eq!(
            kind,
            Kind::Claude,
            "only the harness the workflow names is probed"
        );
        Probe {
            installed: true,
            models: None,
        }
    }
    let (host, _, _) = store_host();
    let mut host = host.with_probe(claude_only);
    let nodes: Vec<Value> = definition_nodes()
        .into_iter()
        .map(|mut n| {
            if n["type"] == "session" {
                n["harness"] = json!("claude");
            }
            n
        })
        .collect();
    let result = handle(
        "workflows.add",
        &Json::from(&json!({"workflow": definition(json!({"nodes": nodes}))})),
        &mut host,
    )
    .unwrap();
    assert!(list(&result, "issues").is_empty(), "{}", joined(&result));
}

#[test]
fn signals_track_running_workflows_across_clones_of_the_handle() {
    let signals = Arc::new(Signals::default());
    let other = signals.clone();
    assert!(!other.is_running("wf-1"));
    signals.set_running("wf-1", true);
    assert!(other.is_running("wf-1"));
    signals.set_running("wf-1", false);
    assert!(!other.is_running("wf-1"));
    other.request_check_now();
    assert!(signals.take_check_now());
    assert!(!signals.take_check_now());
}

#[test]
fn a_check_request_wakes_whoever_waits_for_it() {
    let signals = Arc::new(Signals::default());
    // Nobody waiting yet: still only a flag.
    signals.request_check_now();
    assert!(signals.take_check_now());
    let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = woken.clone();
    signals.set_waker(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    signals.request_check_now();
    signals.request_check_now();
    assert_eq!(woken.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(signals.take_check_now());
}

#[test]
fn handle_value_takes_and_gives_serde_json() {
    let mut f = Fx::new();
    let out = handle_value(
        "workflows.add",
        &json!({"workflow": manual_workflow()}),
        &mut f.host,
    )
    .unwrap();
    assert_eq!(out["enabled"], false);
    assert!(out["id"].as_str().unwrap().starts_with("wf-"));
    assert_eq!(
        handle_value("workflows.nope", &json!({}), &mut f.host).unwrap_err(),
        "Unknown app action: workflows.nope"
    );
}

#[test]
fn the_model_form_the_mono_is_told_resolves_and_is_what_react_stores() {
    // HELP and the context say: `<harness>:<id from models.list>`, Claude without `claude-`.
    for native in ["claude-sonnet-5", "claude-opus-5", "claude-opus-4-6"] {
        let stored = format!("claude:{}", native.strip_prefix("claude-").unwrap());
        assert_eq!(
            crate::store::native_model(&stored, "claude").as_deref(),
            Some(native),
            "{stored}"
        );
    }
    let mut f = Fx::new();
    for model in ["claude:sonnet-5", "claude:opus-5"] {
        let nodes: Vec<Value> = definition_nodes()
            .into_iter()
            .map(|mut n| {
                if n["type"] == "session" {
                    n["model"] = json!(model);
                }
                n
            })
            .collect();
        let result = f.add(definition(json!({"nodes": nodes})));
        assert!(
            list(&result, "issues").is_empty(),
            "{model}: {}",
            joined(&result)
        );
    }
}

#[test]
fn add_probes_the_catalog_before_the_save() {
    struct Order(Memory, Vec<&'static str>);
    impl WorkflowHost for Order {
        fn load(&mut self) -> Result<Vec<Workflow>, String> {
            self.0.load()
        }
        fn update<T>(
            &mut self,
            mut edit: impl FnMut(&mut Self, Vec<Workflow>) -> Result<(Vec<Workflow>, T), String>,
        ) -> Result<T, String> {
            self.1.push("update");
            let (next, result) = edit(self, self.0.stored.clone())?;
            self.0.stored = next;
            Ok(result)
        }
        fn runs(&mut self, id: &str) -> Result<Vec<WorkflowRun>, String> {
            self.0.runs(id)
        }
        fn clear_runs(&mut self, id: &str) {
            self.0.clear_runs(id)
        }
        fn catalog(&mut self, _: &Workflow) -> ModelCatalog {
            self.1.push("catalog");
            catalog()
        }
        fn is_running(&self, id: &str) -> bool {
            self.0.is_running(id)
        }
        fn request_check_now(&mut self) {}
        fn now(&self) -> u64 {
            now()
        }
        fn preflight(&mut self, w: &Workflow) -> Preflight {
            self.1.push("preflight");
            self.0.preflight(w)
        }
        fn project_path(&self, input: &str) -> Option<String> {
            self.0.project_path(input)
        }
    }
    let mut host = Order(Memory::default(), Vec::new());
    handle(
        "workflows.add",
        &Json::from(&json!({"workflow": definition(json!({}))})),
        &mut host,
    )
    .unwrap();
    assert_eq!(host.1, ["catalog", "update"]);
}

// ---- requires ---------------------------------------------------------------------------

#[test]
fn add_stores_requires_and_get_shows_them() {
    let mut f = Fx::new();
    let requires =
        json!([{"cli": "gh", "hint": "brew install gh"}, {"env": "TOKEN"}, {"mcp": "docs server"}]);
    let id = id_of(&f.add(definition(json!({"requires": requires}))));
    let saved = &f.stored()[0];
    assert_eq!(saved.requires.len(), 3);
    assert_eq!(saved.requires[0].hint.as_deref(), Some("brew install gh"));
    let got = f.ok("workflows.get", json!({"id": id}));
    assert_eq!(got.get("requires").unwrap().to_value(), requires);
}

#[test]
fn add_reports_an_unsafe_requirement_as_an_issue() {
    let mut f = Fx::new();
    let added = f.add(definition(json!({"requires": [{"cli": "gh; rm -rf ~"}]})));
    assert!(
        joined(&added).contains("Invalid cli requirement"),
        "{}",
        joined(&added)
    );
}

#[test]
fn add_refuses_a_malformed_requires_instead_of_dropping_it() {
    let mut f = Fx::new();
    for bad in [
        json!("gh"),
        json!(["gh"]),
        json!([{"hint": "x"}]),
        json!([{"cli": "gh", "mcp": "x"}]),
        json!([{"cli": 1}]),
        json!([{"cli": "gh", "hint": 1}]),
        json!([{"cli": "gh", "hnit": "x"}]),
        json!([{"cli": "gh", "version": "1"}]),
        json!(null),
    ] {
        let e = f.err(
            "workflows.add",
            json!({"workflow": definition(json!({"requires": bad.clone()}))}),
        );
        assert!(e.contains("requires"), "{bad}: {e}");
    }
    assert!(f.stored().is_empty());
}

#[test]
fn update_keeps_requires_when_omitted_clears_with_an_empty_list_and_replaces_with_a_new_one() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({"requires": [{"cli": "gh"}]}))));
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"name": "Renamed"}))}),
    );
    assert_eq!(f.stored()[0].requires.len(), 1);
    assert_eq!(f.stored()[0].requires[0].name, "gh");
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"requires": [{"env": "TOKEN"}]}))}),
    );
    assert_eq!(f.stored()[0].requires.len(), 1);
    assert_eq!(f.stored()[0].requires[0].name, "TOKEN");
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"requires": []}))}),
    );
    assert!(f.stored()[0].requires.is_empty());
}

#[test]
fn validate_with_an_id_judges_the_stored_requires_when_none_are_given() {
    let mut f = Fx::new();
    let mut stored = workflow_stub("wf-dup");
    stored.requires = vec![
        Requirement {
            kind: RequirementKind::Cli,
            name: "gh".into(),
            hint: None,
        },
        Requirement {
            kind: RequirementKind::Cli,
            name: "gh".into(),
            hint: None,
        },
    ];
    f.seed(vec![stored]);
    for extra in [json!({}), json!({"inputValues": {"project": "/code/app"}})] {
        let checked = f.ok(
            "workflows.validate",
            json!({"id": "wf-dup", "workflow": definition(extra)}),
        );
        assert!(
            joined(&checked).contains("Duplicate requirement"),
            "{}",
            joined(&checked)
        );
    }
    // An explicit list is judged as given.
    let cleared = f.ok(
        "workflows.validate",
        json!({"id": "wf-dup", "workflow": definition(json!({"requires": []}))}),
    );
    assert!(!joined(&cleared).contains("Duplicate requirement"));
}

#[test]
fn validate_judges_requires_like_add() {
    let mut f = Fx::new();
    let checked = f.ok(
        "workflows.validate",
        json!({"workflow": definition(json!({"requires": [{"env": "1BAD"}]}))}),
    );
    assert!(joined(&checked).contains("Invalid env requirement"));
}

#[test]
fn yaml_add_carries_requires_and_keeps_the_mark() {
    let (mut f, yaml) = imported();
    let yaml = yaml.replacen("nodes:", "requires:\n  - cli: gh\nnodes:", 1);
    let added = f.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    );
    let _ = added;
    assert_eq!(f.stored()[0].requires.len(), 1);
    assert!(f.stored()[0].needs_review);
}

#[test]
fn update_of_a_needs_review_workflow_keeps_the_mark_and_takes_requires() {
    let (mut f, yaml) = imported();
    let id = id_of(&f.ok(
        "workflows.add",
        json!({"yaml": yaml, "values": {"project": "/code/app"}}),
    ));
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"requires": [{"cli": "gh"}]}))}),
    );
    assert!(f.stored()[0].needs_review && !f.stored()[0].enabled);
    assert_eq!(f.stored()[0].requires.len(), 1);
}

// ---- run values, start gates and validate warnings --------------------------------------

const SECRET: &str = "sentinel-secret-value-123";

fn missing(kind: RequirementKind, name: &str, hint: Option<&str>) -> Preflight {
    Preflight {
        statuses: vec![RequirementStatus {
            requirement: Requirement {
                kind,
                name: name.into(),
                hint: hint.map(str::to_string),
            },
            met: false,
            detail: Some("Not found on PATH".into()),
        }],
    }
}

/// An enabled workflow with a project input, a number input and optional extras.
fn enabled_with_count(f: &mut Fx, extra: Value) -> String {
    let mut base = json!({
        "inputs": {"project": {"type": "project"}, "count": {"type": "number"}},
        "inputValues": {"project": "/code/app", "count": 2}
    });
    for (k, v) in extra.as_object().unwrap() {
        base[k] = v.clone();
    }
    let id = id_of(&f.add(definition(base)));
    f.ok("workflows.resume", json!({"id": id}));
    id
}

fn tail_json(error: &str, prefix: &str) -> Value {
    let tail = error
        .strip_prefix(prefix)
        .unwrap_or_else(|| panic!("{error}"));
    serde_json::from_str(tail).unwrap_or_else(|e| panic!("{e}: {tail}"))
}

const MISSING_PREFIX: &str = "The workflow cannot run yet. Missing requirements: ";

#[test]
fn run_values_are_kept_for_the_run_and_never_written_to_the_input_values() {
    let mut f = Fx::new();
    let id = enabled_with_count(&mut f, json!({}));
    f.ok("workflows.run", json!({"id": id, "values": {"count": 7}}));
    let saved = &f.stored()[0];
    assert!(saved.run_requested);
    assert_eq!(
        saved.run_values.as_ref().unwrap().get("count"),
        Some(&InputValue::Number(7.0))
    );
    assert_eq!(
        saved.input_values.get("count"),
        Some(&InputValue::Number(2.0))
    );
    assert_eq!(f.host.checks, 1);
}

#[test]
fn run_without_values_leaves_no_run_values() {
    let mut f = Fx::new();
    let id = enabled_with_count(&mut f, json!({}));
    f.ok("workflows.run", json!({"id": id, "values": {}}));
    assert!(f.stored()[0].run_requested);
    assert!(f.stored()[0].run_values.is_none());
}

#[test]
fn run_values_of_the_wrong_type_or_an_unknown_name_are_refused_and_nothing_is_requested() {
    let mut f = Fx::new();
    let id = enabled_with_count(&mut f, json!({}));
    let updates = f.host.updates;
    let e = f.err(
        "workflows.run",
        json!({"id": id, "values": {"count": "many"}}),
    );
    assert!(
        e.contains("must be a number") && e.contains("Declared inputs: "),
        "{e}"
    );
    let e = f.err("workflows.run", json!({"id": id, "values": {"nope": 1}}));
    assert!(e.contains("Unknown input \"nope\""), "{e}");
    let e = f.err("workflows.run", json!({"id": id, "values": "x"}));
    assert!(e.contains("values must be an object"), "{e}");
    assert!(!f.stored()[0].run_requested && f.stored()[0].run_values.is_none());
    assert_eq!((f.host.updates, f.host.checks), (updates, 0));
}

#[test]
fn a_project_value_must_resolve_to_an_assigned_project() {
    let mut f = Fx::new();
    let id = enabled_with_count(&mut f, json!({}));
    f.host.projects = Some(vec!["/code/app".into(), "/code/other".into()]);
    let e = f.err(
        "workflows.run",
        json!({"id": id, "values": {"project": "/elsewhere/x"}}),
    );
    assert_eq!(
        e,
        "Choose one of this Mono's assigned projects by full path"
    );
    assert!(!f.stored()[0].run_requested && f.stored()[0].run_values.is_none());
    // A folder name resolves to the full path, which is what the run sees.
    f.ok(
        "workflows.run",
        json!({"id": id, "values": {"project": "other"}}),
    );
    assert_eq!(
        f.stored()[0].run_values.as_ref().unwrap().get("project"),
        Some(&InputValue::String("/code/other".into()))
    );
}

#[test]
fn run_blocked_by_a_missing_requirement_ends_in_parseable_json_and_requests_nothing() {
    let mut f = Fx::new();
    let id = enabled_with_count(&mut f, json!({"requires": [{"cli": "gh"}]}));
    f.host.preflight = Some(missing(RequirementKind::Cli, "gh", Some("brew install gh")));
    let updates = f.host.updates;
    let e = f.err("workflows.run", json!({"id": id, "values": {"count": 3}}));
    let list = tail_json(&e, MISSING_PREFIX);
    assert_eq!(
        list,
        json!([{"kind": "cli", "name": "gh", "detail": "Not found on PATH", "hint": "brew install gh"}])
    );
    assert!(!f.stored()[0].run_requested && f.stored()[0].run_values.is_none());
    assert_eq!((f.host.updates, f.host.checks), (updates, 0));
    assert!(!e.contains(SECRET));
}

#[test]
fn run_checks_requirements_on_the_workflow_with_the_run_values_laid_over() {
    let mut f = Fx::new();
    let id = enabled_with_count(
        &mut f,
        json!({"requires": [{"cli": "gh"}, {"env": "TOKEN"}]}),
    );
    f.ok("workflows.run", json!({"id": id}));
    assert_eq!(
        f.host.preflights,
        vec![vec!["gh".to_string(), "TOKEN".to_string()]]
    );
}

#[test]
fn a_workflow_without_requires_is_never_preflighted() {
    let mut f = Fx::new();
    let id = enabled_with_count(&mut f, json!({}));
    f.host.preflight = Some(missing(RequirementKind::Cli, "gh", None));
    f.ok("workflows.run", json!({"id": id}));
    f.ok("workflows.get", json!({"id": id}));
    assert!(f.host.preflights.is_empty());
}

#[test]
fn needs_review_and_paused_still_win_over_a_missing_requirement() {
    let mut f = Fx::new();
    let id = enabled_with_count(&mut f, json!({"requires": [{"cli": "gh"}]}));
    f.host.preflight = Some(missing(RequirementKind::Cli, "gh", None));
    f.host.stored[0].needs_review = true;
    assert_eq!(f.err("workflows.run", json!({"id": id})), NEEDS_REVIEW_TEXT);
    f.host.stored[0].needs_review = false;
    f.ok("workflows.pause", json!({"id": id}));
    assert_eq!(
        f.err("workflows.run", json!({"id": id})),
        "Resume the workflow before running it."
    );
    assert!(f.host.preflights.is_empty());
}

const NEEDS_REVIEW_TEXT: &str = "This workflow came from a file or from a Mono and the user has not reviewed it yet. Ask the user to review it and enable it in Workflows.";

#[test]
fn run_does_not_preflight_inside_the_hash_checked_update() {
    // add() computes problems first for the same reason: the edit may run twice.
    struct Order(Memory, Vec<&'static str>);
    impl WorkflowHost for Order {
        fn load(&mut self) -> Result<Vec<Workflow>, String> {
            self.0.load()
        }
        fn update<T>(
            &mut self,
            mut edit: impl FnMut(&mut Self, Vec<Workflow>) -> Result<(Vec<Workflow>, T), String>,
        ) -> Result<T, String> {
            self.1.push("update");
            let (next, result) = edit(self, self.0.stored.clone())?;
            self.0.stored = next;
            Ok(result)
        }
        fn runs(&mut self, id: &str) -> Result<Vec<WorkflowRun>, String> {
            self.0.runs(id)
        }
        fn clear_runs(&mut self, id: &str) {
            self.0.clear_runs(id)
        }
        fn catalog(&mut self, _: &Workflow) -> ModelCatalog {
            catalog()
        }
        fn is_running(&self, id: &str) -> bool {
            self.0.is_running(id)
        }
        fn request_check_now(&mut self) {}
        fn now(&self) -> u64 {
            now()
        }
        fn preflight(&mut self, w: &Workflow) -> Preflight {
            self.1.push("preflight");
            self.0.preflight(w)
        }
        fn project_path(&self, input: &str) -> Option<String> {
            self.0.project_path(input)
        }
    }
    let mut seed = Fx::new();
    let id = enabled_with_count(&mut seed, json!({"requires": [{"cli": "gh"}]}));
    let mut host = Order(seed.host, Vec::new());
    handle("workflows.run", &Json::from(&json!({"id": id})), &mut host).unwrap();
    assert_eq!(host.1, ["preflight", "update"]);
}

#[test]
fn resume_run_is_blocked_by_what_the_run_or_the_workflow_requires() {
    let mut f = Fx::new();
    let id = id_of(&f.add(definition(json!({"requires": [{"env": "TOKEN"}]}))));
    let mut stored_run = run("run-1", RunStatus::Interrupted, &id);
    stored_run.definition.requires = vec![Requirement {
        kind: RequirementKind::Cli,
        name: "gh".into(),
        hint: None,
    }];
    f.host.runs.insert(id.clone(), vec![stored_run]);
    f.host.preflight = Some(missing(RequirementKind::Cli, "gh", None));
    let e = f.err("workflows.resumeRun", json!({"id": id, "runId": "run-1"}));
    assert_eq!(tail_json(&e, MISSING_PREFIX)[0]["name"], "gh");
    assert_eq!(f.stored()[0].resume_run_id, None);
    assert_eq!(f.host.checks, 0);
    // The run's own list and the live one are both asked about.
    assert_eq!(
        f.host.preflights,
        vec![vec!["gh".to_string(), "TOKEN".to_string()]]
    );
    f.host.preflight = None;
    f.ok("workflows.resumeRun", json!({"id": id, "runId": "run-1"}));
    assert_eq!(f.stored()[0].resume_run_id.as_deref(), Some("run-1"));
}

#[test]
fn enabling_and_saving_stay_ungated_by_a_missing_requirement() {
    let mut f = Fx::new();
    f.host.preflight = Some(missing(RequirementKind::Cli, "gh", None));
    let id = id_of(&f.add(definition(json!({"requires": [{"cli": "gh"}]}))));
    f.ok("workflows.resume", json!({"id": id}));
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"requires": [{"cli": "gh"}]}))}),
    );
    f.ok(
        "workflows.validate",
        json!({"workflow": definition(json!({"requires": [{"cli": "gh"}]}))}),
    );
    assert!(f.stored()[0].enabled);
    assert!(f.host.preflights.is_empty());
}

#[test]
fn get_shows_the_status_of_each_requirement_under_requires_status() {
    let mut f = Fx::new();
    let requires = json!([{"cli": "gh", "hint": "brew install gh"}, {"env": "TOKEN"}]);
    let id = id_of(&f.add(definition(json!({"requires": requires}))));
    f.host.preflight = Some(Preflight {
        statuses: vec![
            RequirementStatus {
                requirement: Requirement {
                    kind: RequirementKind::Cli,
                    name: "gh".into(),
                    hint: Some("brew install gh".into()),
                },
                met: false,
                detail: Some("Not found on PATH".into()),
            },
            RequirementStatus {
                requirement: Requirement {
                    kind: RequirementKind::Env,
                    name: "TOKEN".into(),
                    hint: None,
                },
                met: true,
                detail: None,
            },
        ],
    });
    let got = f.ok("workflows.get", json!({"id": id}));
    assert_eq!(got.get("requires").unwrap().to_value(), requires);
    assert_eq!(
        got.get("requiresStatus").unwrap().to_value(),
        json!([
            {"kind": "cli", "name": "gh", "hint": "brew install gh", "met": false, "detail": "Not found on PATH"},
            {"kind": "env", "name": "TOKEN", "met": true}
        ])
    );
    assert!(!got.stringify(false).contains(SECRET));
}

fn command_flow(command: &str, requires: Value) -> Value {
    definition(json!({
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "build", "type": "command", "project": "{{inputs.project}}", "command": command}
        ],
        "edges": [{"from": "start", "to": "build"}],
        "requires": requires
    }))
}

#[test]
fn validate_warns_about_undeclared_clis_in_command_steps_without_blocking() {
    let mut f = Fx::new();
    let result = f.ok(
        "workflows.validate",
        json!({"workflow": command_flow("gh pr list | jq . && ls", json!([{"cli": "gh"}]))}),
    );
    assert!(list(&result, "issues").is_empty());
    let warnings = list(&result, "warnings");
    assert_eq!(warnings.len(), 1);
    assert_eq!(text(&warnings[0], "nodeId"), "build");
    assert_eq!(text(&warnings[0], "cli"), "jq");
    assert!(text(&warnings[0], "message").contains("jq"));
    // Nothing declared: both CLIs are named; a base utility never is.
    let result = f.ok(
        "workflows.validate",
        json!({"workflow": command_flow("gh pr list | jq . && ls", json!([]))}),
    );
    let clis: Vec<String> = list(&result, "warnings")
        .iter()
        .map(|w| text(w, "cli"))
        .collect();
    assert_eq!(clis, ["gh", "jq"]);
    // No command step, no warnings; and add still saves it.
    let result = f.ok(
        "workflows.validate",
        json!({"workflow": definition(json!({}))}),
    );
    assert!(list(&result, "warnings").is_empty());
    f.add(command_flow("gh pr list", json!([])));
    assert_eq!(f.stored().len(), 1);
}

#[test]
fn a_secret_in_the_environment_never_reaches_a_result_a_run_file_or_a_failure_text() {
    use super::fixtures::SecretProbe;
    use super::preflight::evaluate;
    let probe = SecretProbe(SECRET);
    let requires = json!([{"env": "TOKEN"}, {"env": "LOGIN_TOKEN"}, {"env": "MISSING", "hint": "export MISSING"}, {"cli": "gh"}, {"mcp": "docs"}]);
    let mut f = Fx::new();
    // A command step, so the login shell is consulted for LOGIN_TOKEN.
    let flow = command_flow("true", json!([]));
    let id = enabled_with_count(
        &mut f,
        json!({"requires": requires, "nodes": flow["nodes"], "edges": flow["edges"]}),
    );
    // The host answers from a real evaluation over the secret-holding machine.
    f.host.preflight = Some(evaluate(&f.host.stored[0].clone(), &probe));
    let judged = f.host.preflight.clone().unwrap();
    assert!(!judged.all_met());
    let status_of = |name: &str| {
        judged
            .statuses
            .iter()
            .find(|s| s.requirement.name == name)
            .unwrap()
    };
    // Consulted: the login shell answered for LOGIN_TOKEN (it prints the secret too, filtered out).
    assert!(status_of("TOKEN").met && status_of("LOGIN_TOKEN").met);
    // The failed MCP listing carries the secret in its error; only a fixed text may surface.
    assert_eq!(
        status_of("docs").detail.as_deref(),
        Some("Could not list MCP servers")
    );
    let mut seen: Vec<String> = Vec::new();
    seen.push(f.err("workflows.run", json!({"id": id, "values": {"count": 3}})));
    let mut interrupted = run("run-1", RunStatus::Interrupted, &id);
    interrupted.definition.requires = f.host.stored[0].requires.clone();
    f.host.runs.insert(id.clone(), vec![interrupted]);
    seen.push(f.err("workflows.resumeRun", json!({"id": id, "runId": "run-1"})));
    seen.push(f.ok("workflows.get", json!({"id": id})).stringify(false));
    seen.push(f.ok("workflows.list", json!({})).stringify(false));
    seen.push(f.ok("workflows.runs", json!({"id": id})).stringify(false));
    for text in &seen {
        assert!(!text.contains(SECRET), "leaked: {text}");
    }
    // The checks did run: the unset variable and the missing CLI are named, the set ones are not.
    let blocked = &seen[0];
    assert!(
        blocked.contains("MISSING") && blocked.contains("export MISSING") && blocked.contains("gh")
    );
    // The same machine behind a tick: the failed run's file and the chat text carry no value.
    let root = std::env::temp_dir().join(format!("wf-secret-{}", crate::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    let disk = WorkflowsDir::new(root.join("workflows"));
    let mut w = f.host.stored[0].clone();
    w.run_requested = true;
    super::store::update_workflows(&disk, |_| Ok((vec![w.clone()], ()))).unwrap();
    let judge = |w: &Workflow| evaluate(w, &probe);
    let found = super::tick::check(
        &disk,
        &super::tick::Tick {
            now: now(),
            live_runs: &|_| 0,
            probe: no_probe,
            on_claim: &|_, _| {},
            preflight: &judge,
        },
    )
    .unwrap();
    let super::tick::Start::Run { run, failure } = &found.starts[0] else {
        panic!("no run")
    };
    assert_eq!(run.status, RunStatus::Failed);
    let failure = failure.as_ref().unwrap();
    assert!(failure.text.contains("MISSING"));
    super::store::update_runs(&disk, &id, |_| Ok((vec![run.clone()], ()))).unwrap();
    let mut files = String::new();
    fn gather(dir: &std::path::Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                gather(&path, out);
            } else {
                out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    gather(&root, &mut files);
    assert!(
        files.contains("Missing requirements"),
        "run file was written"
    );
    for text in [&failure.text, &failure.title, &files] {
        assert!(!text.contains(SECRET), "leaked: {text}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_store_host_refuses_a_run_value_project_outside_the_assigned_projects() {
    let (host, _, root) = store_host();
    let mut host = host.with_caller(MonoCaller {
        id: "mono-1".into(),
        projects: vec!["/code/app".into(), "/code/other".into()],
    });
    assert_eq!(host.project_path("other").as_deref(), Some("/code/other"));
    assert_eq!(host.project_path("/code/app").as_deref(), Some("/code/app"));
    assert_eq!(host.project_path("/elsewhere/x"), None);
    let wf = json!({
        "name": "Build",
        "inputs": {"project": {"type": "project"}},
        "inputValues": {"project": "/code/app"},
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "build", "type": "command", "project": "{{inputs.project}}", "command": "true"}
        ],
        "edges": [{"from": "start", "to": "build"}]
    });
    let mut call = |action: &str, input: Value| handle(action, &Json::from(&input), &mut host);
    let id = id_of(&call("workflows.add", json!({"workflow": wf})).unwrap());
    // What a Mono made waits for the user's review; here the user has approved it.
    let dir = WorkflowsDir::new(root.join("workflows"));
    update_workflows(&dir, |mut list| {
        list[0].needs_review = false;
        Ok((list, ()))
    })
    .unwrap();
    call("workflows.resume", json!({"id": id})).unwrap();
    let e = call(
        "workflows.run",
        json!({"id": id, "values": {"project": "/elsewhere/x"}}),
    )
    .unwrap_err();
    assert_eq!(
        e,
        "Choose one of this Mono's assigned projects by full path"
    );
    assert!(!load_workflows(&dir).unwrap().workflows[0].run_requested);
    call(
        "workflows.run",
        json!({"id": id, "values": {"project": "other"}}),
    )
    .unwrap();
    let saved = &load_workflows(&dir).unwrap().workflows[0];
    assert_eq!(
        saved.run_values.as_ref().unwrap().get("project"),
        Some(&InputValue::String("/code/other".into()))
    );
    assert_eq!(
        saved.input_values.get("project"),
        Some(&InputValue::String("/code/app".into()))
    );
    // Without a caller a value is accepted as given.
    let (plain, _, _) = store_host();
    assert_eq!(
        plain.project_path("/anywhere").as_deref(),
        Some("/anywhere")
    );
}

// ---- cloud targets ------------------------------------------------------------------------

fn cloud_definition(first: Value, second: Value) -> Value {
    let mut nodes = definition_nodes();
    let fetch = nodes.iter_mut().find(|n| n["id"] == "fetch").unwrap();
    fetch["target"] = first;
    nodes.insert(
        2,
        json!({
            "id": "next", "type": "session", "project": "{{inputs.project}}",
            "prompt": "go on", "checkout": "root", "output": {}, "target": second
        }),
    );
    definition(json!({
        "nodes": nodes,
        "edges": [
            {"from": "start", "to": "fetch"},
            {"from": "fetch", "to": "next"}
        ]
    }))
}

#[test]
fn validate_add_and_update_report_cloud_target_issues() {
    let mut f = Fx::new();
    let good = cloud_definition(
        json!({"kind": "codespace", "branch": "main"}),
        json!({"kind": "codespace", "instance": {"sameAs": "fetch"}}),
    );
    let ok = f.ok("workflows.validate", json!({"workflow": good}));
    assert_eq!(messages(&ok), Vec::<String>::new());
    let bad = cloud_definition(
        json!({"kind": "codespace"}),
        json!({"kind": "sandbox", "instance": {"sameAs": "fetch"}}),
    );
    let validated = f.ok("workflows.validate", json!({"workflow": bad}));
    assert!(joined(&validated).contains("must be the same kind"));
    let added = f.add(bad.clone());
    assert_eq!(validated.get("issues"), added.get("issues"));
    let id = id_of(&added);
    let updated = f.ok("workflows.update", json!({"id": id, "workflow": bad}));
    assert_eq!(validated.get("issues"), updated.get("issues"));
}

#[test]
fn a_target_that_is_not_an_object_names_the_node_in_the_error() {
    let mut f = Fx::new();
    for target in [json!("codespace"), json!({"instance": "new"})] {
        let e = f.err(
            "workflows.validate",
            json!({"workflow": cloud_definition(target, json!({"kind": "codespace"}))}),
        );
        assert!(e.contains("nodes[1] (\"fetch\")"), "{e}");
        assert!(e.contains("wrong type"), "{e}");
    }
}

// ---- the new definition fields and the global limit --------------------------------------

#[test]
fn add_and_update_take_a_decision_model_and_a_summary_template_and_keep_them_when_left_out() {
    let mut f = Fx::new();
    let created = f.add(definition(json!({
        "decisionModel": {"harness": "claude", "model": "claude:sonnet-5"},
        "summaryTemplate": "Done: {{fetch.tickets}}"
    })));
    let id = id_of(&created);
    assert!(list(&created, "issues").is_empty(), "{}", joined(&created));
    let stored = f.stored()[0].clone();
    assert_eq!(
        stored.decision_model.as_ref().unwrap().model.as_deref(),
        Some("claude:sonnet-5")
    );
    assert_eq!(
        stored.summary_template.as_deref(),
        Some("Done: {{fetch.tickets}}")
    );
    // An update that says nothing about them keeps them; one that gives them replaces them.
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"name": "Renamed"}))}),
    );
    let kept = f.stored()[0].clone();
    assert_eq!(kept.name, "Renamed");
    assert_eq!(kept.decision_model, stored.decision_model);
    assert_eq!(kept.summary_template, stored.summary_template);
    f.ok(
        "workflows.update",
        json!({"id": id, "workflow": definition(json!({"summaryTemplate": "New"}))}),
    );
    assert_eq!(f.stored()[0].summary_template.as_deref(), Some("New"));
    assert_eq!(f.stored()[0].decision_model, stored.decision_model);
    // A decision model needs a harness; a summary a non-blank text.
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"decisionModel": {"model": "claude:sonnet-5"}}))}),
    );
    assert!(e.contains("workflow.decisionModel must be"), "{e}");
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"decisionModel": {"harness": "claude", "model": 3}}))}),
    );
    assert!(e.contains("workflow.decisionModel must be"), "{e}");
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"summaryTemplate": "  "}))}),
    );
    assert!(
        e.contains("workflow.summaryTemplate must be a non-empty string"),
        "{e}"
    );
}

#[test]
fn a_workflow_with_several_triggers_validates_and_lists_them_all() {
    let mut f = Fx::new();
    let mut def = definition(json!({}));
    def["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "hand", "type": "manual"}));
    def["edges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"from": "hand", "to": "fetch"}));
    let created = f.add(def);
    assert!(list(&created, "issues").is_empty(), "{}", joined(&created));
    let got = f.ok("workflows.get", json!({"id": id_of(&created)}));
    let kinds: Vec<String> = list(&got, "triggers")
        .iter()
        .map(|t| text(t, "type"))
        .collect();
    assert_eq!(kinds, ["schedule", "manual"]);
    // One that leads somewhere else is an issue.
    let mut bad = definition(json!({}));
    bad["nodes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "hand", "type": "manual"}));
    let result = f.add(bad);
    assert!(
        joined(&result).contains("same first step"),
        "{}",
        joined(&result)
    );
}

// ---- the global store with a Mono as the caller -----------------------------------------

const OLD_REPORT_FILE: &str = "monoptahWorkflow: 1
name: Old
inputs:
  project:
    type: project
nodes:
  - id: start
    type: manual
  - id: dev
    type: session
    project: \"{{inputs.project}}\"
    checkout: root
    prompt: Go
    output:
      note:
        type: string
  - id: report
    type: report
    template: \"Result: {{dev.note}}\"
edges:
  - from: start
    to: dev
  - from: dev
    to: report
";

/// A definition whose only project is `project`, given as the value of its project input (a
/// step can only name a project through an input).
fn in_project(project: &str) -> Value {
    json!({
        "name": "Literal",
        "inputs": {"project": {"type": "project"}},
        "inputValues": {"project": project},
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "work", "type": "session", "project": "{{inputs.project}}", "prompt": "Go", "checkout": "root"}
        ],
        "edges": [{"from": "start", "to": "work"}]
    })
}

/// A host seen from Mono `mono-1`, which works on /code/app only, with `/code/app` and
/// `/code/other` workflows already stored (reviewed and enabled, as the user left them).
fn mono_fx() -> (Fx, String, String) {
    let mut f = Fx::new();
    let mine = id_of(&f.add(in_project("/code/app")));
    let theirs = id_of(&f.add(in_project("/code/other")));
    for w in f.host.stored.iter_mut() {
        w.needs_review = false;
        w.enabled = true;
    }
    f.host.caller = Some(MonoCaller {
        id: "mono-1".into(),
        projects: vec!["/code/app".into()],
    });
    (f, mine, theirs)
}

#[test]
fn a_mono_lists_gets_and_reads_runs_of_every_workflow() {
    let (mut f, mine, theirs) = mono_fx();
    let listed = f.ok("workflows.list", json!({}));
    assert_eq!(list(&listed, "workflows").len(), 2);
    for id in [&mine, &theirs] {
        assert_eq!(text(&f.ok("workflows.get", json!({"id": id})), "id"), *id);
        f.ok("workflows.runs", json!({"id": id}));
    }
}

#[test]
fn a_mono_only_updates_and_runs_workflows_whose_projects_are_its_own() {
    let (mut f, mine, theirs) = mono_fx();
    let change = |name: &str, project: &str| {
        let mut w = in_project(project);
        w["name"] = json!(name);
        w
    };
    f.ok("workflows.run", json!({"id": mine}));
    assert!(f.stored().iter().any(|w| w.id == mine && w.run_requested));
    f.ok(
        "workflows.update",
        json!({"id": mine, "workflow": change("Renamed", "/code/app")}),
    );
    for e in [
        f.err(
            "workflows.update",
            json!({"id": theirs, "workflow": change("X", "/code/other")}),
        ),
        f.err(
            "workflows.update",
            json!({"id": mine, "workflow": change("Y", "/code/other")}),
        ),
        f.err("workflows.run", json!({"id": theirs})),
    ] {
        assert!(
            e.contains("not this Mono's") && e.contains("/code/other"),
            "{e}"
        );
    }
    assert!(f
        .stored()
        .iter()
        .all(|w| w.id != theirs || !w.run_requested));
    assert_eq!(
        f.stored().iter().find(|w| w.id == mine).unwrap().name,
        "Renamed"
    );
}

#[test]
fn a_mono_only_pauses_resumes_stops_and_removes_workflows_on_its_own_projects() {
    let (mut f, mine, theirs) = mono_fx();
    f.host.live.insert(mine.clone());
    f.host.live.insert(theirs.clone());
    for action in ["pause", "resume", "stop", "remove"] {
        let e = f.err(&format!("workflows.{action}"), json!({"id": theirs}));
        assert!(
            e.contains("not this Mono's") && e.contains("/code/other"),
            "{action}: {e}"
        );
    }
    let other = f.stored().iter().find(|w| w.id == theirs).unwrap().clone();
    assert!(other.enabled && !other.stop_requested);
    f.ok("workflows.stop", json!({"id": mine}));
    f.host.live.remove(&mine);
    f.ok("workflows.pause", json!({"id": mine}));
    assert!(!f.stored().iter().find(|w| w.id == mine).unwrap().enabled);
    f.ok("workflows.resume", json!({"id": mine}));
    assert!(f.stored().iter().find(|w| w.id == mine).unwrap().enabled);
    f.ok("workflows.remove", json!({"id": mine}));
    assert!(f.stored().iter().all(|w| w.id != mine));
    f.host.live.remove(&theirs);
    f.err("workflows.remove", json!({"id": theirs}));
    assert!(f.stored().iter().any(|w| w.id == theirs));
}

#[test]
fn a_mono_resumes_a_run_only_when_the_runs_own_snapshot_is_in_scope() {
    let (mut f, mine, _) = mono_fx();
    // The workflow now points at the Mono's project, but the run was made on another one.
    let mut stored_run = run("run-1", RunStatus::Interrupted, &mine);
    let mut snapshot = f.stored().iter().find(|w| w.id == mine).unwrap().clone();
    let mut run_values = OrderedMap::new();
    run_values.insert("project", InputValue::String("/code/other".into()));
    snapshot.run_values = Some(run_values);
    stored_run.definition = snapshot.clone();
    f.host.runs.insert(mine.clone(), vec![stored_run.clone()]);
    let e = f.err("workflows.resumeRun", json!({"id": mine, "runId": "run-1"}));
    assert!(
        e.contains("not this Mono's") && e.contains("/code/other"),
        "{e}"
    );
    assert_eq!(f.stored()[0].resume_run_id, None);
    snapshot.run_values = None;
    stored_run.definition = snapshot;
    f.host.runs.insert(mine.clone(), vec![stored_run]);
    f.ok("workflows.resumeRun", json!({"id": mine, "runId": "run-1"}));
}

#[test]
fn a_string_input_used_as_a_step_project_counts_as_a_project() {
    let (mut f, _, _) = mono_fx();
    let mut w = in_project("/code/other");
    w["inputs"] = json!({"project": {"type": "string"}});
    let e = f.err("workflows.add", json!({"workflow": w}));
    assert!(
        e.contains("not this Mono's") && e.contains("/code/other"),
        "{e}"
    );
}

#[test]
fn a_mono_changing_the_definition_of_an_approved_workflow_sends_it_back_for_review() {
    let (mut f, mine, _) = mono_fx();
    let mut edited = in_project("/code/app");
    edited["nodes"][1]["prompt"] = json!("Go, but differently");
    let result = f.ok("workflows.update", json!({"id": mine, "workflow": edited}));
    assert!(flag(&result, "needsReview"));
    let w = f.stored().iter().find(|w| w.id == mine).unwrap();
    assert!(w.needs_review && !w.enabled);
    assert_review(&f.err("workflows.resume", json!({"id": mine})));
}

#[test]
fn a_mono_changing_only_input_values_leaves_an_approved_workflow_alone() {
    let (mut f, mine, _) = mono_fx();
    f.ok(
        "workflows.setInputs",
        json!({"id": mine, "values": {"project": "/code/app"}}),
    );
    let mut same = in_project("/code/app");
    same["inputValues"] = json!({"project": "/code/app"});
    f.ok("workflows.update", json!({"id": mine, "workflow": same}));
    let w = f.stored().iter().find(|w| w.id == mine).unwrap();
    assert!(!w.needs_review && w.enabled);
}

#[test]
fn a_users_own_edit_of_an_approved_workflow_does_not_need_review() {
    let (mut f, mine, _) = mono_fx();
    f.host.caller = None;
    let mut edited = in_project("/code/app");
    edited["nodes"][1]["prompt"] = json!("Go, but differently");
    f.ok("workflows.update", json!({"id": mine, "workflow": edited}));
    let w = f.stored().iter().find(|w| w.id == mine).unwrap();
    assert!(!w.needs_review && w.enabled);
}

#[test]
fn a_mono_adds_only_workflows_on_its_own_projects_and_they_are_saved_paused_for_review() {
    let (mut f, _, _) = mono_fx();
    let e = f.err(
        "workflows.add",
        json!({"workflow": in_project("/code/other")}),
    );
    assert!(e.contains("not this Mono's"), "{e}");
    let added = f.add(in_project("/code/app"));
    assert!(!flag(&added, "enabled"));
    let made = f.stored().iter().find(|w| w.id == id_of(&added)).unwrap();
    assert!(!made.enabled && made.needs_review);
    // A Mono cannot enable it, and the user's own add through the app does not need review.
    let mut enable = in_project("/code/app");
    enable["enabled"] = json!(true);
    let made_id = made.id.clone();
    assert_eq!(
        f.err(
            "workflows.update",
            json!({"id": made_id, "workflow": enable})
        ),
        NEEDS_REVIEW_TEXT
    );
    f.host.caller = None;
    let theirs = f.add(in_project("/code/other"));
    let user_made = f.stored().iter().find(|w| w.id == id_of(&theirs)).unwrap();
    assert!(!user_made.needs_review);
}

#[test]
fn a_project_a_step_takes_from_a_previous_step_cannot_be_checked_so_a_mono_may_not_use_it() {
    let (mut f, _, _) = mono_fx();
    let mut w = in_project("/code/app");
    w["nodes"] = json!([
        {"id": "start", "type": "manual"},
        {"id": "fetch", "type": "command", "project": "/code/app", "command": "pwd"},
        {"id": "work", "type": "session", "project": "{{fetch.dir}}", "prompt": "Go", "checkout": "root"}
    ]);
    w["edges"] = json!([{"from": "start", "to": "fetch"}, {"from": "fetch", "to": "work"}]);
    let e = f.err("workflows.add", json!({"workflow": w}));
    assert!(e.contains("previous step"), "{e}");
}

#[test]
fn a_mono_run_records_the_mono_as_the_caller() {
    let (mut f, mine, _) = mono_fx();
    f.ok("workflows.run", json!({"id": mine}));
    let w = f.stored().iter().find(|w| w.id == mine).unwrap();
    let caller = w.run_caller.clone().expect("a caller is recorded");
    assert_eq!(caller.kind, CallerKind::Mono);
    assert_eq!(caller.mono_id.as_deref(), Some("mono-1"));
    assert_eq!(caller.trigger_node, "start");
    // The user's run records nothing: the tick starts it as manual.
    f.host.caller = None;
    f.host.stored.iter_mut().for_each(|w| w.run_caller = None);
    f.host
        .stored
        .iter_mut()
        .for_each(|w| w.run_requested = false);
    f.ok("workflows.run", json!({"id": mine}));
    assert!(f
        .stored()
        .iter()
        .find(|w| w.id == mine)
        .unwrap()
        .run_caller
        .is_none());
}

#[test]
fn report_to_accepts_a_mono_naming_itself_and_refuses_it_naming_another() {
    let (mut f, _, _) = mono_fx();
    let mut own = in_project("/code/app");
    own["reportTo"] = json!({"mono": "mono-1"});
    let id = id_of(&f.add(own));
    let saved = f.stored().iter().find(|w| w.id == id).unwrap();
    assert_eq!(
        saved.report_to.as_ref().and_then(|r| r.mono.as_deref()),
        Some("mono-1")
    );
    let mut other = in_project("/code/app");
    other["reportTo"] = json!({"mono": "mono-2"});
    let e = f.err("workflows.add", json!({"workflow": other.clone()}));
    assert!(e.contains("reportTo.mono can only be this Mono"), "{e}");
    let mine = f.stored()[0].id.clone();
    let e = f.err(
        "workflows.update",
        json!({"id": mine, "workflow": other.clone()}),
    );
    assert!(e.contains("reportTo.mono"), "{e}");
    // The user may name any Mono.
    f.host.caller = None;
    let id = id_of(&f.add(other));
    let saved = f.stored().iter().find(|w| w.id == id).unwrap();
    assert_eq!(
        saved.report_to.as_ref().and_then(|r| r.mono.as_deref()),
        Some("mono-2")
    );
}

#[test]
fn an_old_per_mono_id_gets_the_clear_error_and_an_unknown_one_does_not() {
    let (mut f, _, _) = mono_fx();
    f.host.legacy.insert("old-1".into());
    for action in [
        "workflows.run",
        "workflows.get",
        "workflows.runs",
        "workflows.stop",
    ] {
        assert_eq!(
            f.err(action, json!({"id": "old-1"})),
            "This workflow was per Mono and is no longer used; recreate it in Workflows"
        );
    }
    let e = f.err("workflows.run", json!({"id": "never-existed"}));
    assert!(e.contains("No workflow with that id"), "{e}");
}

#[test]
fn a_node_called_trigger_is_refused_because_trigger_is_a_template_variable() {
    let mut f = Fx::new();
    let added = f.add(json!({
        "name": "Clash",
        "nodes": [
            {"id": "trigger", "type": "manual"},
            {"id": "work", "type": "command", "project": "/code/app", "command": "true"}
        ],
        "edges": [{"from": "trigger", "to": "work"}]
    }));
    assert!(
        joined(&added).contains("Node id \"trigger\" is reserved"),
        "{}",
        joined(&added)
    );
}

#[test]
fn a_yaml_import_shows_what_the_import_changed() {
    let mut f = Fx::new();
    let added = f.ok(
        "workflows.add",
        json!({"yaml": OLD_REPORT_FILE, "values": {"project": "/code/app"}}),
    );
    let warnings = list(&added, "importWarnings");
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0]
        .as_str()
        .unwrap()
        .contains("The Report node \"report\" was removed"));
    let checked = f.ok("workflows.validate", json!({"yaml": OLD_REPORT_FILE}));
    assert_eq!(list(&checked, "importWarnings").len(), 1);
}

// ---- workspace, concurrency, the queue ---------------------------------------------------

fn running_workflow(f: &mut Fx, extra: Value) -> String {
    let id = id_of(&f.add(definition(extra)));
    f.ok("workflows.resume", json!({"id": id}));
    id
}

fn queued(id: &str, workflow: &str, at: u64) -> WorkflowRun {
    let mut run = run(id, RunStatus::Queued, workflow);
    run.nodes.clear();
    run.queued_at = Some(at);
    run.started_at = at;
    run
}

#[test]
fn add_reads_the_workspace_and_concurrency_and_get_returns_them() {
    let mut f = Fx::new();
    let result = f.add(definition(json!({
        "workspace": {"mode": "perRun", "project": "{{inputs.project}}", "baseBranch": "develop"},
        "concurrency": {"max": 5}
    })));
    assert!(
        list(&result, "issues").is_empty(),
        "{}",
        result.stringify(false)
    );
    let saved = &f.stored()[0];
    assert_eq!(saved.workspace.mode, WorkspaceMode::PerRun);
    assert_eq!(saved.workspace.project, "{{inputs.project}}");
    assert_eq!(saved.workspace.base(), "develop");
    assert_eq!(saved.concurrency.limit(), 5);
    let id = saved.id.clone();
    let got = f.ok("workflows.get", json!({"id": id}));
    assert_eq!(
        got.get("workspace").unwrap().to_value(),
        json!({"mode": "perRun", "project": "{{inputs.project}}", "baseBranch": "develop"})
    );
    assert_eq!(
        got.get("concurrency").unwrap().to_value(),
        json!({"max": 5})
    );
}

#[test]
fn a_workflow_without_them_defaults_to_a_per_run_workspace_and_three_runs_at_once() {
    let mut f = Fx::new();
    f.add(without(definition(json!({})), "workspace"));
    let saved = &f.stored()[0];
    assert_eq!(saved.workspace, Workspace::default());
    assert_eq!(saved.workspace.mode, WorkspaceMode::PerRun);
    assert_eq!(saved.workspace.base(), "main");
    assert_eq!(saved.concurrency.limit(), 3);
    // Nothing is written for the defaults.
    let id = saved.id.clone();
    let got = f.ok("workflows.get", json!({"id": id}));
    assert!(got.get("workspace").is_none() && got.get("concurrency").is_none());
}

#[test]
fn a_bad_workspace_or_concurrency_is_refused_or_reported() {
    let mut f = Fx::new();
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"workspace": {"mode": "everywhere"}}))}),
    );
    assert!(e.contains("workflow.workspace must be"), "{e}");
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({"concurrency": "lots"}))}),
    );
    assert!(e.contains("workflow.concurrency must be"), "{e}");
    let result = f.add(definition(json!({"concurrency": {"max": 11}})));
    assert!(joined(&result).contains("concurrency.max must be a whole number from 1 to 10"));
}

#[test]
fn validate_warns_when_steps_that_can_run_at_the_same_time_write_into_one_worktree() {
    let mut f = Fx::new();
    let parallel = definition(json!({
        "workspace": {"mode": "perRun", "project": "{{inputs.project}}"},
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "a", "type": "session", "prompt": "x"},
            {"id": "b", "type": "session", "prompt": "y"}
        ],
        "edges": [{"from": "start", "to": "a"}, {"from": "start", "to": "b"}]
    }));
    let result = f.ok("workflows.validate", json!({"workflow": parallel}));
    assert!(
        list(&result, "issues").is_empty(),
        "{}",
        result.stringify(false)
    );
    let warnings: Vec<String> = list(&result, "warnings")
        .iter()
        .map(|w| text(w, "message"))
        .collect();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("\"a\" and \"b\" can run at the same time"));
    // One after the other is fine.
    let in_order = definition(json!({
        "workspace": {"mode": "perRun", "project": "{{inputs.project}}"},
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "a", "type": "session", "prompt": "x"},
            {"id": "b", "type": "session", "prompt": "y"}
        ],
        "edges": [{"from": "start", "to": "a"}, {"from": "a", "to": "b"}]
    }));
    let result = f.ok("workflows.validate", json!({"workflow": in_order}));
    assert!(list(&result, "warnings").is_empty());
}

#[test]
fn run_starts_now_when_a_slot_is_free_and_reports_the_queue_place_when_not() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({"concurrency": {"max": 2}}));
    f.host.live_n.insert(id.clone(), 1);
    let result = f.ok("workflows.run", json!({"id": id}));
    assert!(!flag(&result, "queued"));
    assert_eq!(text(&result, "note"), "It starts now, on its own.");
    // A request nobody picked up yet is not asked twice.
    let e = f.err("workflows.run", json!({"id": id}));
    assert!(e.contains("already waiting to start"), "{e}");
    f.host.stored[0].run_requested = false;
    // Both slots taken, one run already waits: this one is second.
    f.host.live_n.insert(id.clone(), 2);
    f.host.runs.insert(id.clone(), vec![queued("q1", &id, 1)]);
    let result = f.ok("workflows.run", json!({"id": id}));
    assert!(flag(&result, "queued"));
    assert_eq!(result.get("position").and_then(Json::as_f64), Some(2.0));
    assert!(text(&result, "note").contains("place 2"));
}

#[test]
fn run_is_refused_when_fifty_runs_already_wait() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({"concurrency": {"max": 1}}));
    f.host.live.insert(id.clone());
    f.host.runs.insert(
        id.clone(),
        (0..50)
            .map(|n| queued(&format!("q{n}"), &id, n + 1))
            .collect(),
    );
    let e = f.err("workflows.run", json!({"id": id}));
    assert!(e.contains("50 runs waiting"), "{e}");
    assert!(!f.stored()[0].run_requested);
}

#[test]
fn stop_with_several_runs_going_names_them_and_a_run_id_stops_one() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({}));
    f.host.live_n.insert(id.clone(), 2);
    f.host.runs.insert(
        id.clone(),
        vec![
            run("run-b", RunStatus::Running, &id),
            run("run-a", RunStatus::Running, &id),
        ],
    );
    let e = f.err("workflows.stop", json!({"id": id}));
    assert!(
        e.contains("2 runs") && e.contains("run-a") && e.contains("run-b"),
        "{e}"
    );
    assert!(!f.stored()[0].stop_requested && f.stored()[0].stop_run_ids.is_empty());
    f.ok("workflows.stop", json!({"id": id, "runId": "run-a"}));
    assert_eq!(f.stored()[0].stop_run_ids, vec!["run-a".to_string()]);
    assert!(!f.stored()[0].stop_requested);
    assert_eq!(f.host.checks, 1);
    // A run that is not there or not going is refused.
    let e = f.err("workflows.stop", json!({"id": id, "runId": "nobody"}));
    assert!(e.contains("No run with that id"), "{e}");
}

#[test]
fn stop_with_exactly_one_run_going_stops_it_without_a_run_id() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({}));
    f.host.live.insert(id.clone());
    f.host
        .runs
        .insert(id.clone(), vec![run("run-a", RunStatus::Running, &id)]);
    f.ok("workflows.stop", json!({"id": id}));
    assert!(f.stored()[0].stop_requested);
}

#[test]
fn stop_with_a_queued_run_id_cancels_it_and_never_touches_the_runs_that_go() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({}));
    f.host.live.insert(id.clone());
    f.host.runs.insert(
        id.clone(),
        vec![queued("q1", &id, 5), run("run-a", RunStatus::Running, &id)],
    );
    let result = f.ok("workflows.stop", json!({"id": id, "runId": "q1"}));
    assert!(text(&result, "note").contains("never started"));
    assert_eq!(f.host.cancelled, vec![(id.clone(), "q1".to_string())]);
    assert!(!f.stored()[0].stop_requested && f.stored()[0].stop_run_ids.is_empty());
    // With nothing going and runs waiting, a stop without an id says which ones wait.
    f.host.live.clear();
    let e = f.err("workflows.stop", json!({"id": id}));
    assert!(e.contains("only queued ones") && e.contains("q1"), "{e}");
}

#[test]
fn resume_run_needs_a_free_slot_not_an_idle_workflow() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({"concurrency": {"max": 2}}));
    f.host
        .runs
        .insert(id.clone(), vec![run("run-1", RunStatus::Interrupted, &id)]);
    f.host.live_n.insert(id.clone(), 2);
    let e = f.err("workflows.resumeRun", json!({"id": id, "runId": "run-1"}));
    assert!(e.contains("2 runs going"), "{e}");
    f.host.live_n.insert(id.clone(), 1);
    f.ok("workflows.resumeRun", json!({"id": id, "runId": "run-1"}));
    assert_eq!(f.stored()[0].resume_run_id.as_deref(), Some("run-1"));
}

#[test]
fn remove_is_refused_while_runs_go_or_wait() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({}));
    f.host.live_n.insert(id.clone(), 2);
    let e = f.err("workflows.remove", json!({"id": id}));
    assert!(e.contains("Stop the workflow"), "{e}");
    f.host.live_n.clear();
    f.host
        .runs
        .insert(id.clone(), vec![queued("q2", &id, 6), queued("q1", &id, 5)]);
    let e = f.err("workflows.remove", json!({"id": id}));
    assert!(e.contains("2 queued runs"), "{e}");
    assert_eq!(f.stored().len(), 1);
    assert!(f.host.cleared.is_empty());
    f.host.runs.clear();
    f.ok("workflows.remove", json!({"id": id}));
    assert!(f.stored().is_empty());
}

#[test]
fn runs_report_their_number_worktree_and_queue_place() {
    let mut f = Fx::new();
    let id = running_workflow(&mut f, json!({}));
    let mut done = run("run-1", RunStatus::Done, &id);
    done.number = Some(4);
    done.worktree = Some(RunWorktree {
        path: "/code/app-worktrees/wf-x-4".into(),
        branch: "wf/x-4".into(),
        base: "abc".into(),
        removed_at: None,
    });
    f.host.runs.insert(
        id.clone(),
        vec![queued("q2", &id, 6), queued("q1", &id, 5), done],
    );
    let result = f.ok("workflows.runs", json!({"id": id}));
    let runs = list(&result, "runs");
    assert_eq!(
        runs[0].get("queuePosition").and_then(Json::as_f64),
        Some(2.0)
    );
    assert_eq!(
        runs[1].get("queuePosition").and_then(Json::as_f64),
        Some(1.0)
    );
    assert_eq!(runs[2].get("number").and_then(Json::as_f64), Some(4.0));
    assert_eq!(
        runs[2].get("worktree").unwrap().to_value(),
        json!({"path": "/code/app-worktrees/wf-x-4", "branch": "wf/x-4"})
    );
}

#[test]
fn a_mono_that_changes_the_workspace_or_concurrency_hands_the_workflow_back_for_review() {
    let (mut f, mine, _) = mono_fx();
    let definition = in_project("/code/app");
    for change in [
        json!({"concurrency": {"max": 8}}),
        json!({"workspace": {"mode": "none"}}),
    ] {
        f.host.stored[0].needs_review = false;
        f.host.stored[0].enabled = true;
        f.host.stored[0].workspace = Workspace::default();
        f.host.stored[0].concurrency = Concurrency::default();
        let mut next = definition.clone();
        for (k, v) in change.as_object().unwrap() {
            next[k] = v.clone();
        }
        let result = f.ok("workflows.update", json!({"id": mine, "workflow": next}));
        assert_eq!(
            result.get("needsReview"),
            Some(&Json::Bool(true)),
            "{change}"
        );
        assert!(f.host.stored[0].needs_review && !f.host.stored[0].enabled);
    }
}

#[test]
fn the_workspace_project_counts_in_a_monos_project_scope() {
    let (mut f, _, _) = mono_fx();
    // The steps are the Mono's, the workspace project is not: refused.
    let e = f.err(
        "workflows.add",
        json!({"workflow": definition(json!({
            "workspace": {"mode": "perRun", "project": "/code/other"}
        }))}),
    );
    assert!(
        e.contains("/code/other") && e.contains("not this Mono's"),
        "{e}"
    );
    // A project input as the workspace project is covered by the input's value.
    f.add(definition(json!({
        "workspace": {"mode": "perRun", "project": "{{inputs.project}}"}
    })));
}

#[test]
fn the_store_host_cancels_a_queued_run_in_the_file_and_counts_live_runs_per_run() {
    let (mut host, signals, root) = store_host();
    let dir = WorkflowsDir::new(root.join("workflows"));
    let id = id_of(
        &handle(
            "workflows.add",
            &Json::from(&json!({"workflow": manual_workflow()})),
            &mut host,
        )
        .unwrap(),
    );
    handle(
        "workflows.resume",
        &Json::from(&json!({"id": id})),
        &mut host,
    )
    .unwrap();
    let mut waiting = run("q-1", RunStatus::Queued, &id);
    waiting.nodes.clear();
    waiting.queued_at = Some(5);
    super::store::update_runs(&dir, &id, |_| Ok((vec![waiting.clone()], ()))).unwrap();
    // Two runs of the workflow go: each counts, and it is running until both are released.
    signals.claim_run(&id, "run-a");
    signals.claim_run(&id, "run-b");
    assert_eq!(signals.live_runs(&id), 2);
    signals.release_run(&id, "run-a");
    assert!(signals.is_running(&id));
    signals.release_run(&id, "run-b");
    assert!(!signals.is_running(&id));
    // A stop with the queued run's id cancels it in the file, with nothing going.
    let result = handle(
        "workflows.stop",
        &Json::from(&json!({"id": id, "runId": "q-1"})),
        &mut host,
    )
    .unwrap();
    assert!(text(&result, "note").contains("never started"));
    let stored = load_runs(&dir, &id).unwrap();
    assert_eq!(stored[0].status, RunStatus::Stopped);
    assert!(stored[0].ended_at.is_some());
    // Cancelled once: a second try says it is no longer queued.
    let again = handle(
        "workflows.stop",
        &Json::from(&json!({"id": id, "runId": "q-1"})),
        &mut host,
    )
    .unwrap_err();
    assert!(again.contains("not going or queued"), "{again}");
}
