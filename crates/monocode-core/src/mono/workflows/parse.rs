//! Lenient reading of workflow files. A node or edge that cannot be read could not run, and
//! dropping it quietly would change what the workflow does, so the whole entry is left out.
use super::event::EventTrigger;
use super::model::{
    CallerKind, Checkout, Concurrency, Cwd, Decide, DecisionModel, Edge, Handle, InputType,
    InputValue, LoopSpec, Node, NodeKind, OutputField, OutputFieldType, ReportTo, Requirement,
    RequirementKind, RunCaller, RunRequest, SessionTarget, TargetInstance, Ui, Workflow,
    WorkflowInput, WorkflowUi, Workspace, WorkspaceMode, DECIDE_AI, DIRECTION_LR, NODE_TYPES,
};
use super::ordered::{Json, OrderedMap};
use crate::mono::habits::Schedule;
use std::collections::HashSet;

fn str_of(v: &Json, key: &str) -> Option<String> {
    v.get(key).and_then(Json::as_str).map(str::to_string)
}
fn num_of(v: &Json, key: &str) -> Option<f64> {
    v.get(key).and_then(Json::as_f64)
}
fn bool_of(v: &Json, key: &str) -> Option<bool> {
    v.get(key).and_then(Json::as_bool)
}
fn is_rec(v: &Json) -> bool {
    matches!(v, Json::Object(_))
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`
pub fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `name` may be a key of an input or output map. Names end up in `{{a.b}}` paths, and
/// "__proto__" is refused for parity with the React app, which builds plain objects.
pub fn is_map_key(name: &str) -> bool {
    is_name(name) && name != "__proto__"
}

/// A number that does not fit the habit schedule's integer fields becomes the largest value, so
/// validation still reports it with the habit's own message instead of the draft being dropped.
fn small_whole(n: f64, max: f64) -> f64 {
    if n.fract() == 0.0 && (0.0..=max).contains(&n) {
        n
    } else {
        max
    }
}

fn parse_schedule(raw: Option<&Json>) -> Option<Schedule> {
    let raw = raw.filter(|r| is_rec(r))?;
    let kind = str_of(raw, "scheduleKind").filter(|k| !k.is_empty())?;
    Some(Schedule {
        // Whether the kind is one a habit knows is validate_workflow's call, so a draft survives.
        schedule_kind: kind,
        time: str_of(raw, "time").unwrap_or_else(|| "09:00".into()),
        minute: small_whole(num_of(raw, "minute").unwrap_or(0.0), u32::MAX as f64) as u32,
        day_of_week: small_whole(num_of(raw, "dayOfWeek").unwrap_or(1.0), u32::MAX as f64) as u32,
        interval_hours: num_of(raw, "intervalHours").map(|n| small_whole(n, u8::MAX as f64) as u8),
        interval_from: str_of(raw, "intervalFrom"),
    })
}

fn worktree_of(raw: &Json) -> Option<String> {
    raw.get("worktreeOf")
        .and_then(Json::as_str)
        .map(str::to_string)
}

/// Absent means `run`, the run's own worktree, and `root` for a cloud step (which works in its
/// instance's own checkout). None for a value that is not a checkout.
fn parse_checkout(raw: Option<&Json>, cloud: bool) -> Option<Checkout> {
    match raw {
        None | Some(Json::Null) if cloud => Some(Checkout::Root),
        None | Some(Json::Null) => Some(Checkout::Run),
        Some(Json::String(s)) if s == "run" => Some(Checkout::Run),
        Some(Json::String(s)) if s == "root" => Some(Checkout::Root),
        Some(Json::String(s)) if s == "newWorktree" => Some(Checkout::NewWorktree),
        Some(other @ Json::Object(_)) => match other.get("existing") {
            Some(Json::String(path)) if !path.is_empty() => Some(Checkout::Existing(path.clone())),
            Some(_) => None,
            None => worktree_of(other).map(Checkout::WorktreeOf),
        },
        _ => None,
    }
}

/// A step's `project`: absent is empty (the workflow's `workspace.project`), a value that is
/// not text is unreadable.
fn parse_project(raw: &Json) -> Option<String> {
    match raw.get("project") {
        None | Some(Json::Null) => Some(String::new()),
        Some(Json::String(project)) => Some(project.clone()),
        Some(_) => None,
    }
}

/// `Some(None)` when there is no `workspace`, `None` when it is there but unreadable (it decides
/// where a run works, so it is never dropped quietly).
pub(crate) fn parse_workspace(raw: Option<&Json>) -> Option<Workspace> {
    let raw = match raw {
        None | Some(Json::Null) => return Some(Workspace::default()),
        Some(raw) if is_rec(raw) => raw,
        Some(_) => return None,
    };
    let mode = match raw.get("mode") {
        None | Some(Json::Null) => WorkspaceMode::PerRun,
        Some(Json::String(name)) => WorkspaceMode::ALL
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, mode)| *mode)?,
        Some(_) => return None,
    };
    let project = match raw.get("project") {
        None | Some(Json::Null) => String::new(),
        Some(Json::String(project)) => project.clone(),
        Some(_) => return None,
    };
    let base_branch = match raw.get("baseBranch") {
        None | Some(Json::Null) => None,
        Some(Json::String(branch)) => Some(branch.clone()),
        Some(_) => return None,
    };
    Some(Workspace {
        mode,
        project,
        base_branch,
    })
}

/// The workflow's `concurrency`; None when it is there but unreadable.
pub(crate) fn parse_concurrency(raw: Option<&Json>) -> Option<Concurrency> {
    let raw = match raw {
        None | Some(Json::Null) => return Some(Concurrency::default()),
        Some(raw) if is_rec(raw) => raw,
        Some(_) => return None,
    };
    match raw.get("max") {
        None | Some(Json::Null) => Some(Concurrency::default()),
        Some(Json::Number(max)) => Some(Concurrency { max: *max }),
        Some(_) => None,
    }
}

/// `Some(None)` when there is no account, `None` when it is there but is not text: dropping it
/// would run the step with the default account, which pays for the instance, so the whole
/// entry is left out instead (like an unreadable `target`).
fn parse_account(raw: Option<&Json>) -> Option<Option<String>> {
    match raw {
        None | Some(Json::Null) => Some(None),
        Some(Json::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    }
}

/// `Some(None)` when there is no target, `None` when it is there but unreadable.
fn parse_target(raw: Option<&Json>) -> Option<Option<SessionTarget>> {
    let Some(raw) = raw else {
        return Some(None);
    };
    let fields = raw.as_object()?;
    let kind = str_of(raw, "kind")?;
    let instance = match fields.get("instance") {
        None => TargetInstance::New,
        Some(Json::String(s)) if s == "new" => TargetInstance::New,
        Some(other) => match other.as_object() {
            Some(o) if o.len() == 1 => match o.get("sameAs") {
                Some(Json::String(id)) if !id.is_empty() => TargetInstance::SameAs(id.clone()),
                _ => TargetInstance::Invalid(other.clone()),
            },
            _ => TargetInstance::Invalid(other.clone()),
        },
    };
    let mut options = OrderedMap::new();
    for (key, value) in fields.iter() {
        if key != "kind" && key != "instance" {
            options.push_unique(key.clone(), value.clone());
        }
    }
    Some(Some(SessionTarget {
        kind,
        instance,
        options,
    }))
}

fn parse_output(raw: Option<&Json>) -> Option<OrderedMap<OutputField>> {
    let Some(raw) = raw else {
        return Some(OrderedMap::new());
    };
    let fields = raw.as_object()?;
    let mut output = OrderedMap::new();
    for (name, field) in fields.iter() {
        let ty = field.get("type").and_then(Json::as_str).and_then(|t| {
            OutputFieldType::ALL
                .iter()
                .find(|(n, _)| *n == t)
                .map(|(_, ty)| *ty)
        });
        let (true, Some(ty)) = (is_map_key(name) && is_rec(field), ty) else {
            continue;
        };
        output.push_unique(
            name.clone(),
            OutputField {
                ty,
                in_report: bool_of(field, "inReport"),
                description: str_of(field, "description"),
            },
        );
    }
    Some(output)
}

/// Whether `value` asks the AI to decide. "mono" is what files from before AI decisions wrote.
pub fn is_ai_decide(value: &str) -> bool {
    value == DECIDE_AI || value == "mono"
}

fn parse_decide(raw: &Json) -> Option<Decide> {
    str_of(raw, "decide")
        .is_some_and(|d| is_ai_decide(&d))
        .then_some(Decide::Ai)
}

/// An event trigger. A missing or unknown source or event is kept (empty) so validation can
/// name it; a filter of the wrong type makes the node unreadable.
fn parse_event(raw: &Json) -> Option<EventTrigger> {
    let project = match raw.get("project") {
        None | Some(Json::Null) => None,
        Some(Json::String(p)) => Some(p.clone()).filter(|p| !p.trim().is_empty()),
        Some(_) => return None,
    };
    let repos = match raw.get("repos") {
        None | Some(Json::Null) => Vec::new(),
        Some(Json::Array(items)) => items
            .iter()
            .map(|r| r.as_str().map(|r| r.trim().to_string()))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .filter(|r| !r.is_empty())
            .collect(),
        Some(_) => return None,
    };
    Some(EventTrigger {
        source: str_of(raw, "source").unwrap_or_default(),
        event: str_of(raw, "event").unwrap_or_default(),
        project,
        repos,
    })
}

pub fn parse_node(raw: &Json) -> Option<Node> {
    if !is_rec(raw) {
        return None;
    }
    let id = str_of(raw, "id").filter(|id| !id.is_empty())?;
    let ty = raw.get("type").and_then(Json::as_str)?;
    if !NODE_TYPES.contains(&ty) {
        return None;
    }
    let ui = raw.get("ui").and_then(|ui| {
        Some(Ui {
            x: num_of(ui, "x")?,
            y: num_of(ui, "y")?,
        })
    });
    let kind = match ty {
        "schedule" => NodeKind::Schedule {
            schedule: parse_schedule(raw.get("schedule"))?,
        },
        "manual" => NodeKind::Manual,
        "event" => NodeKind::Event(parse_event(raw)?),
        "session" => {
            let target = parse_target(raw.get("target"))?;
            NodeKind::Session {
                project: parse_project(raw)?,
                prompt: str_of(raw, "prompt")?,
                checkout: parse_checkout(raw.get("checkout"), target.is_some())?,
                // `outputs` is what is written; `output` is what older files and the first design say.
                output: parse_output(raw.get("outputs").or_else(|| raw.get("output")))?,
                mode: str_of(raw, "mode"),
                surface: str_of(raw, "surface"),
                harness: str_of(raw, "harness"),
                model: str_of(raw, "model"),
                effort: str_of(raw, "effort"),
                reuse_session: bool_of(raw, "reuseSession"),
                nest_under: str_of(raw, "nestUnder"),
                timeout_minutes: num_of(raw, "timeoutMinutes"),
                target,
                account: parse_account(raw.get("account"))?,
                access: str_of(raw, "access"),
                folder: str_of(raw, "folder").filter(|f| !f.trim().is_empty()),
                link_work_item: bool_of(raw, "linkWorkItem"),
            }
        }
        "command" => NodeKind::Command {
            project: parse_project(raw)?,
            command: str_of(raw, "command")?,
            cwd: match raw.get("cwd") {
                Some(Json::String(s)) if s == "root" => Some(Cwd::Root),
                Some(o @ Json::Object(_)) => worktree_of(o).map(Cwd::WorktreeOf),
                _ => None,
            },
            // Anything but text is a mistake, not a missing value: the step is unreadable.
            on_failure: match raw.get("onFailure") {
                None | Some(Json::Null) => None,
                Some(Json::String(s)) => Some(s.clone()),
                Some(_) => return None,
            },
            timeout_minutes: num_of(raw, "timeoutMinutes"),
        },
        "if" => NodeKind::If {
            condition: str_of(raw, "condition"),
            decide: parse_decide(raw),
            question: str_of(raw, "question"),
        },
        "switch" => NodeKind::Switch {
            value: str_of(raw, "value"),
            cases: raw
                .get("cases")
                .and_then(Json::as_array)
                .map(|cases| {
                    cases
                        .iter()
                        .filter_map(|c| c.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            decide: parse_decide(raw),
            question: str_of(raw, "question"),
        },
        "forEach" => NodeKind::ForEach {
            list: str_of(raw, "in")?,
            item: str_of(raw, "as"),
            max_parallel: num_of(raw, "maxParallel"),
        },
        _ => return None,
    };
    Some(Node {
        id,
        title: str_of(raw, "title"),
        parent: str_of(raw, "parent"),
        ui,
        kind,
    })
}

pub fn parse_edge(raw: &Json) -> Option<Edge> {
    if !is_rec(raw) {
        return None;
    }
    let from = str_of(raw, "from").filter(|s| !s.is_empty())?;
    let to = str_of(raw, "to").filter(|s| !s.is_empty())?;
    let loop_spec = match raw.get("loop") {
        None => None,
        Some(l) if is_rec(l) => Some(LoopSpec {
            max_rounds: num_of(l, "maxRounds")?,
            decide: {
                let decide = str_of(l, "decide")?;
                if is_ai_decide(&decide) {
                    DECIDE_AI.to_string()
                } else {
                    decide
                }
            },
        }),
        Some(_) => return None,
    };
    let handle = raw
        .get("handle")
        .and_then(Json::as_str)
        .filter(|h| !h.is_empty())
        .map(Handle::named);
    Some(Edge {
        from,
        to,
        handle,
        loop_spec,
    })
}

/// Drops the handles an edge cannot use. A named handle other than "true" or "false" only means
/// something on a case or default edge of a Switch, so on a loop edge or on a node that is not a
/// Switch (or an If, which reports it in validation) it is ignored, as such handles always were.
pub(super) fn drop_stray_handles(nodes: &[Node], edges: &mut [Edge]) {
    for edge in edges {
        if !matches!(edge.handle, Some(Handle::Case(_))) {
            continue;
        }
        let from = nodes.iter().find(|n| n.id == edge.from);
        let keeps = edge.loop_spec.is_none()
            && from
                .is_some_and(|n| matches!(n.kind, NodeKind::Switch { .. } | NodeKind::If { .. }));
        if !keeps {
            edge.handle = None;
        }
    }
}

pub fn parse_inputs(raw: Option<&Json>) -> OrderedMap<WorkflowInput> {
    let mut inputs = OrderedMap::new();
    let Some(map) = raw.and_then(Json::as_object) else {
        return inputs;
    };
    for (name, input) in map.iter() {
        let ty = input.get("type").and_then(Json::as_str).and_then(|t| {
            InputType::ALL
                .iter()
                .find(|(n, _)| *n == t)
                .map(|(_, ty)| *ty)
        });
        let (true, Some(ty)) = (is_map_key(name) && is_rec(input), ty) else {
            continue;
        };
        inputs.push_unique(
            name.clone(),
            WorkflowInput {
                ty,
                description: str_of(input, "description"),
            },
        );
    }
    inputs
}

fn parse_input_values(raw: Option<&Json>) -> OrderedMap<InputValue> {
    let mut values = OrderedMap::new();
    let Some(map) = raw.and_then(Json::as_object) else {
        return values;
    };
    for (name, value) in map.iter() {
        if !is_map_key(name) {
            continue;
        }
        match value {
            Json::String(s) => values.push_unique(name.clone(), InputValue::String(s.clone())),
            Json::Number(n) if n.is_finite() => {
                values.push_unique(name.clone(), InputValue::Number(*n))
            }
            Json::Bool(b) => values.push_unique(name.clone(), InputValue::Bool(*b)),
            _ => {}
        }
    }
    values
}

/// The `requires` list of a definition. Absent is an empty list. An entry that cannot be read
/// is an error: a precondition dropped quietly would change what the workflow guards. A name
/// with unsafe characters is well formed, so it is kept for validation to report.
pub fn read_requires(raw: Option<&Json>) -> Result<Vec<Requirement>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let Json::Array(entries) = raw else {
        return Err("requires must be a list".into());
    };
    let mut out = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let at = index + 1;
        let Json::Object(fields) = entry else {
            return Err(format!(
                "requires entry {at} must be an object like {{cli: gh}}"
            ));
        };
        let mut found = RequirementKind::ALL
            .iter()
            .filter(|kind| fields.contains_key(kind.key()));
        let (Some(kind), None) = (found.next(), found.next()) else {
            return Err(format!(
                "requires entry {at} needs exactly one of cli, env or mcp"
            ));
        };
        if let Some(extra) = fields
            .keys()
            .find(|key| *key != kind.key() && *key != "hint")
        {
            return Err(format!(
                "requires entry {at} has an unknown field \"{extra}\"; use {} and an optional hint",
                kind.key()
            ));
        }
        let Some(name) = fields.get(kind.key()).and_then(Json::as_str) else {
            return Err(format!("requires entry {at}: {} must be text", kind.key()));
        };
        let hint = match fields.get("hint") {
            None => None,
            Some(Json::String(hint)) => Some(hint.clone()),
            Some(_) => return Err(format!("requires entry {at}: hint must be text")),
        };
        out.push(Requirement {
            kind: *kind,
            name: name.to_string(),
            hint,
        });
    }
    Ok(out)
}

/// `Some(None)` when there is no decision model, `None` when it is there but unreadable: dropping
/// it would run the decisions on another model, so the whole entry is left out instead.
pub(crate) fn parse_decision_model(raw: Option<&Json>) -> Option<Option<DecisionModel>> {
    let raw = match raw {
        None | Some(Json::Null) => return Some(None),
        Some(raw) if is_rec(raw) => raw,
        Some(_) => return None,
    };
    let effort = match raw.get("effort") {
        None | Some(Json::Null) => None,
        Some(Json::String(effort)) => Some(effort.clone()),
        Some(_) => return None,
    };
    // Absent, null or empty: the harness's default model, resolved when the run starts. A value
    // that is not text is unreadable, not "no model".
    let model = match raw.get("model") {
        None | Some(Json::Null) => None,
        Some(Json::String(model)) => Some(model.clone()).filter(|m| !m.is_empty()),
        Some(_) => return None,
    };
    Some(Some(DecisionModel {
        harness: str_of(raw, "harness").filter(|h| !h.is_empty())?,
        model,
        effort,
    }))
}

/// `Some(None)` when there is no `reportTo`, `None` when it is there but unreadable (it decides
/// who is told about a run, so it is never dropped quietly).
pub(crate) fn parse_report_to(raw: Option<&Json>) -> Option<Option<ReportTo>> {
    let raw = match raw {
        None | Some(Json::Null) => return Some(None),
        Some(raw) if is_rec(raw) => raw,
        Some(_) => return None,
    };
    let mono = match raw.get("mono") {
        None | Some(Json::Null) => None,
        Some(Json::String(mono)) if !mono.is_empty() => Some(mono.clone()),
        Some(_) => return None,
    };
    Some(Some(ReportTo { mono }))
}

/// The `caller` of a run or the `runCaller` of a workflow. None when it cannot be read.
pub fn parse_caller(raw: &Json) -> Option<RunCaller> {
    if !is_rec(raw) {
        return None;
    }
    let kind = match str_of(raw, "kind")?.as_str() {
        "manual" => CallerKind::Manual,
        "schedule" => CallerKind::Schedule,
        "event" => CallerKind::Event,
        "mono" => CallerKind::Mono,
        _ => return None,
    };
    Some(RunCaller {
        kind,
        mono_id: str_of(raw, "monoId"),
        trigger_node: str_of(raw, "triggerNode")?,
        item: raw.get("item").filter(|i| is_rec(i)).cloned(),
    })
}

/// The workflow-level `ui` object; only a direction of "lr" is kept.
pub fn parse_workflow_ui(raw: Option<&Json>) -> Option<WorkflowUi> {
    let direction = raw?.get("direction")?.as_str()?;
    (direction == DIRECTION_LR).then(WorkflowUi::lr)
}

pub fn parse_workflow(raw: &Json) -> Option<Workflow> {
    if !is_rec(raw) {
        return None;
    }
    let id = str_of(raw, "id").filter(|id| !id.is_empty())?;
    let nodes: Option<Vec<Node>> = raw
        .get("nodes")?
        .as_array()?
        .iter()
        .map(parse_node)
        .collect();
    let edges: Option<Vec<Edge>> = match raw.get("edges").and_then(Json::as_array) {
        Some(edges) => edges.iter().map(parse_edge).collect(),
        None => Some(Vec::new()),
    };
    let (nodes, mut edges) = (nodes?, edges?);
    let requires = read_requires(raw.get("requires")).ok()?;
    drop_stray_handles(&nodes, &mut edges);
    let decision_model = parse_decision_model(raw.get("decisionModel"))?;
    let report_to = parse_report_to(raw.get("reportTo"))?;
    let workspace = parse_workspace(raw.get("workspace"))?;
    let concurrency = parse_concurrency(raw.get("concurrency"))?;
    let flag = |key: &str| raw.get(key) == Some(&Json::Bool(true));
    Some(Workflow {
        id,
        name: str_of(raw, "name").unwrap_or_default(),
        description: str_of(raw, "description"),
        inputs: parse_inputs(raw.get("inputs")),
        input_values: parse_input_values(raw.get("inputValues")),
        requires,
        run_values: raw
            .get("runValues")
            .filter(|v| is_rec(v))
            .map(|v| parse_input_values(Some(v))),
        nodes,
        edges,
        ui: parse_workflow_ui(raw.get("ui")),
        // An entry that says nothing about being enabled stays paused.
        enabled: flag("enabled"),
        created_at: num_of(raw, "createdAt").unwrap_or(0.0) as u64,
        next_run_at: num_of(raw, "nextRunAt").map(|n| n as u64),
        next_run_at_by_node: raw
            .get("nextRunAtByNode")
            .and_then(Json::as_object)
            .map(|times| {
                times
                    .iter()
                    .filter_map(|(id, at)| Some((id.clone(), at.as_f64()? as u64)))
                    .collect()
            })
            .unwrap_or_default(),
        run_requested: flag("runRequested"),
        stop_requested: flag("stopRequested"),
        resume_run_id: str_of(raw, "resumeRunId"),
        last_run_id: str_of(raw, "lastRunId"),
        imported_from: str_of(raw, "importedFrom"),
        needs_review: flag("needsReview"),
        decision_model,
        report_to,
        summary_template: str_of(raw, "summaryTemplate"),
        run_caller: raw.get("runCaller").and_then(parse_caller),
        run_backlog: raw
            .get("runBacklog")
            .and_then(Json::as_array)
            .map(|list| {
                list.iter()
                    .filter(|entry| is_rec(entry))
                    .map(|entry| RunRequest {
                        caller: entry.get("caller").and_then(parse_caller),
                        values: entry
                            .get("values")
                            .filter(|v| is_rec(v))
                            .map(|v| parse_input_values(Some(v))),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        workspace,
        concurrency,
        stop_run_ids: raw
            .get("stopRunIds")
            .and_then(Json::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        run_counter: num_of(raw, "runCounter").map_or(0, |n| n as u64),
    })
}

/// What workflows.json holds: the entries that parse, and the ones that do not, as they were written.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowFile {
    pub workflows: Vec<Workflow>,
    pub unreadable: Vec<Json>,
}

/// Splits workflows.json into readable and unreadable entries. An entry is unreadable when it
/// does not parse as a workflow (a node of a type this app does not know, a damaged field) or
/// repeats an earlier id. None when the text is not a JSON list, so a caller can refuse to
/// overwrite it. A blank text is an empty file.
pub fn read_workflow_file(text: &str) -> Option<WorkflowFile> {
    if super::template::js_trim(text).is_empty() {
        return Some(WorkflowFile {
            workflows: Vec::new(),
            unreadable: Vec::new(),
        });
    }
    let Json::Array(entries) = Json::parse(text)? else {
        return None;
    };
    let mut file = WorkflowFile {
        workflows: Vec::new(),
        unreadable: Vec::new(),
    };
    let mut seen = HashSet::new();
    for raw in entries {
        match parse_workflow(&raw) {
            Some(workflow) if seen.insert(workflow.id.clone()) => file.workflows.push(workflow),
            _ => file.unreadable.push(raw),
        }
    }
    Some(file)
}

/// workflows.json as written by the app; anything malformed is left out.
pub fn parse_workflows(text: &str) -> Vec<Workflow> {
    read_workflow_file(text)
        .map(|f| f.workflows)
        .unwrap_or_default()
}
