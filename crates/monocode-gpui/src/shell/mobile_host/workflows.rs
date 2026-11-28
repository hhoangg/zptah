//! `workflows.list` and `workflows.runs`: the read-only Workflows screen of the web app.
//!
//! Both are plain reads over the global workflow store. A device that sees only some projects
//! sees a workflow only when it sees every project the workflow uses; a project that cannot be
//! worked out from the definition hides the workflow (fail closed). Visibility is decided on the
//! background thread before any run is read, so a hidden workflow and a missing one cost and
//! answer the same. A run's sessions are shown only where the ordinary session rules
//! (`mobile_target`) allow them, under the phone's session ids. Nothing here starts, changes or
//! stops a workflow.
//!
//! What reaches a device is built from an allow-list of keys (see `RUN_KEYS`, `NODE_KEYS`,
//! `CALLER_KEYS`), never by removing the keys that must not: a field a later desktop adds stays
//! on the desktop until it is listed here. Node outputs travel as they are (declared fields and
//! the built-in `reply`, `branch`, `commit`, `changedFiles`, `diffStat`, `stdout`, ...), except
//! that the built-in `worktree` path is dropped and `sessionId` is replaced by the phone's id or
//! dropped.
use super::*;
use monocode_core::mono::workflows::{
    cli::{has_unresolved_project, workflow_projects},
    global::WorkflowsDir,
    model::{NodeKind, NodeRun, Workflow, WorkflowRun, with_run_values},
    store::{last_run_of, load_runs_checked, load_workflows},
};
use std::collections::{HashMap, HashSet};

/// What `workflows.list` shows of a workflow's newest run the device may see.
pub(super) enum LastRun {
    Never,
    Seen(Value, u64),
    /// The runs file could not be read: the history is unknown, which is not "never run".
    Unreadable,
}

/// What the files held, already cut down to what the device may see. Built off the UI thread.
#[allow(clippy::large_enum_variant)]
pub(super) enum Loaded {
    List(Vec<(Workflow, LastRun)>),
    /// The workflow and its runs, or `None` when it is missing or hidden (the same reply).
    Runs(Option<(Workflow, Vec<WorkflowRun>)>),
}

/// The phone's id of each persisted session id a reply names, `None` when the device may not see it.
pub(super) type Links = HashMap<String, Option<String>>;

/// Whether `device` may see a workflow that uses `w`'s projects. `known` are the desktop's
/// projects. A device with no selection sees everything.
pub(super) fn workflow_visible(device: &DeviceContext, known: &[String], w: &Workflow) -> bool {
    let Some(selection) = &device.projects else {
        return true;
    };
    if has_unresolved_project(w) {
        return false;
    }
    workflow_projects(w).0.iter().all(|path| {
        let key = monocode_core::files::path_key(path);
        known
            .iter()
            .find(|known| monocode_core::files::path_key(known) == key)
            .is_some_and(|known| selection.iter().any(|id| *id == project_id(known)))
    })
}

/// The longest string kept in a node output or error, and in a summary. Longer text ends in [`TRUNCATED`].
const MAX_TEXT_BYTES: usize = 4 * 1024;
const MAX_SUMMARY_BYTES: usize = 8 * 1024;
/// Names, titles, ids and item fields.
const MAX_LABEL_BYTES: usize = 256;
const MAX_NODE_SESSIONS: usize = 50;
const MAX_RUN_SESSIONS: usize = 100;
const TRUNCATED: &str = "... [truncated]";
/// The most a `workflows.runs` reply may weigh, far under the link's 8 MiB response limit.
const MAX_REPLY_BYTES: usize = 1024 * 1024;
/// What the reply around the runs (the workflow's id and name, the braces) may take.
const REPLY_RESERVE: usize = 4 * 1024;

/// The keys of a run a device gets (besides the computed ones). No definition, no input values,
/// no paths: those stay on the desktop.
pub(super) const RUN_KEYS: [&str; 9] = [
    "id",
    "status",
    "caller",
    "startedAt",
    "endedAt",
    "durationMs",
    "summary",
    "error",
    "nodes",
];
/// What a node run gets from the stored record. `title`, `type` and `sessions` are added.
pub(super) const NODE_KEYS: [&str; 9] = [
    "nodeId",
    "branch",
    "status",
    "round",
    "startedAt",
    "endedAt",
    "error",
    "decision",
    "output",
];
pub(super) const CALLER_KEYS: [&str; 3] = ["kind", "monoId", "triggerNode"];
const ITEM_KEYS: [&str; 6] = ["source", "kind", "ref", "title", "url", "repo"];

fn cap_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED}", &text[..end])
}

fn capped(text: &str, max: usize) -> Value {
    json!(cap_text(text, max))
}

/// Every string inside `value` cut to `max` bytes.
fn cap_strings(value: &mut Value, max: usize) {
    match value {
        Value::String(text) if text.len() > max => *text = cap_text(text, max),
        Value::Array(items) => items.iter_mut().for_each(|item| cap_strings(item, max)),
        Value::Object(fields) => fields.values_mut().for_each(|item| cap_strings(item, max)),
        _ => {}
    }
}

fn json_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX / 4, |bytes| bytes.len())
}

/// Takes the node outputs out of `run`. Whether there were any.
fn strip_outputs(run: &mut Value) -> bool {
    let mut stripped = false;
    for node in run["nodes"].as_array_mut().into_iter().flatten() {
        if let Some(fields) = node.as_object_mut() {
            stripped |= fields.remove("output").is_some();
        }
    }
    if stripped {
        run["outputsDropped"] = json!(true);
    }
    stripped
}

/// Keeps the runs (newest first) under `budget` bytes. Every string is already capped, so this
/// only chooses what to leave out: first the node outputs of the oldest runs (`outputsDropped` on
/// each run that lost some), then the oldest runs themselves, and at last the tail of the newest
/// run's steps (`nodesTruncated`). Each run is sized once and the total follows incrementally.
pub(super) fn bound_runs(runs: &mut Vec<Value>, budget: usize) {
    let mut sizes: Vec<usize> = runs.iter().map(json_len).collect();
    // The brackets and the commas between runs.
    let total = |sizes: &[usize]| sizes.iter().sum::<usize>() + sizes.len() + 1;
    for at in (0..runs.len()).rev() {
        if total(&sizes) <= budget {
            return;
        }
        if strip_outputs(&mut runs[at]) {
            sizes[at] = json_len(&runs[at]);
        }
    }
    while total(&sizes) > budget && runs.len() > 1 {
        runs.pop();
        sizes.pop();
    }
    if total(&sizes) <= budget {
        return;
    }
    let Some(newest) = runs.first_mut() else {
        return;
    };
    // The flag counts toward the size, so it goes in first.
    newest["nodesTruncated"] = json!(true);
    let mut size = json_len(newest);
    let limit = budget.saturating_sub(2);
    let nodes: Vec<usize> = newest["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().map(json_len).collect())
        .unwrap_or_default();
    let mut keep = nodes.len();
    while size > limit && keep > 0 {
        keep -= 1;
        size = size.saturating_sub(nodes[keep] + 1);
    }
    if let Some(list) = newest["nodes"].as_array_mut() {
        list.truncate(keep);
    }
}

fn triggers_of(w: &Workflow) -> Vec<Value> {
    w.triggers()
        .into_iter()
        .map(|node| {
            let label = match &node.kind {
                NodeKind::Schedule { schedule } => schedule.label(),
                NodeKind::Manual => "Manual".to_owned(),
                other => other.type_name().to_owned(),
            };
            json!({
                "id": cap_text(&node.id, MAX_LABEL_BYTES),
                "type": node.kind.type_name(),
                "label": cap_text(&label, MAX_LABEL_BYTES),
            })
        })
        .collect()
}

fn status_of<T: serde::Serialize>(status: &T) -> Value {
    serde_json::to_value(status).unwrap_or(Value::Null)
}

/// The persisted session ids a loaded reply names, each once.
pub(super) fn session_ids_of(loaded: &Loaded) -> Vec<String> {
    let Loaded::Runs(Some((_, runs))) = loaded else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for node in runs.iter().flat_map(|run| &run.nodes) {
        let output_session = node
            .output
            .as_ref()
            .and_then(|output| serde_json::to_value(output).ok())
            .and_then(|output| output.get("sessionId")?.as_str().map(str::to_owned));
        let rounds = node
            .history
            .iter()
            .flatten()
            .filter_map(|r| r.session_id.clone());
        for id in rounds.chain(node.session_id.clone()).chain(output_session) {
            if seen.insert(id.clone()) {
                ids.push(id);
            }
        }
    }
    ids
}

/// The reply of a read, built from what the files held. Pure: the sessions were resolved before.
pub(super) fn reply_of(device: &DeviceContext, loaded: Loaded, links: &Links) -> Value {
    match loaded {
        Loaded::List(workflows) => {
            let list: Vec<Value> = workflows
                .iter()
                .map(|(w, last)| {
                    let mut entry = json!({
                        "id": cap_text(&w.id, MAX_LABEL_BYTES),
                        "name": cap_text(&w.name, MAX_LABEL_BYTES),
                        "description": w.description.as_deref().map(|d| cap_text(d, 1024)),
                        "enabled": w.enabled,
                        "needsReview": w.needs_review,
                        "triggers": triggers_of(w),
                        "nextRunAt": w.next_run_at,
                        "lastRun": null,
                    });
                    match last {
                        LastRun::Never => {}
                        LastRun::Seen(status, started) => {
                            entry["lastRun"] = json!({"status": status, "startedAt": started});
                        }
                        LastRun::Unreadable => entry["lastRunUnreadable"] = json!(true),
                    }
                    entry
                })
                .collect();
            json!({"workflows": list})
        }
        Loaded::Runs(None) => json!({"workflow": null, "runs": []}),
        Loaded::Runs(Some((workflow, runs))) => {
            let mut runs: Vec<Value> = runs
                .iter()
                .map(|run| run_reply(device, run, links))
                .collect();
            bound_runs(&mut runs, MAX_REPLY_BYTES - REPLY_RESERVE);
            json!({
                "workflow": {
                    "id": cap_text(&workflow.id, MAX_LABEL_BYTES),
                    "name": cap_text(&workflow.name, MAX_LABEL_BYTES),
                },
                "runs": runs,
            })
        }
    }
}

fn link_of(links: &Links, persisted: &str) -> Option<String> {
    links.get(persisted).cloned().flatten()
}

fn push_unique(list: &mut Vec<String>, id: String, max: usize) {
    if list.len() < max && !list.contains(&id) {
        list.push(id);
    }
}

fn run_reply(device: &DeviceContext, run: &WorkflowRun, links: &Links) -> Value {
    let mut sessions: Vec<String> = Vec::new();
    let nodes: Vec<Value> = run
        .nodes
        .iter()
        .map(|node| node_reply(run, node, links, &mut sessions))
        .collect();
    let duration = run
        .ended_at
        .filter(|end| *end != 0)
        .map(|end| end.saturating_sub(run.started_at));
    let summary = run.summary.as_ref().map(|s| {
        json!({
            "title": cap_text(&s.title, MAX_LABEL_BYTES),
            "text": cap_text(&s.text, MAX_SUMMARY_BYTES),
        })
    });
    let mut value = json!({
        "id": cap_text(&run.id, MAX_LABEL_BYTES),
        "status": status_of(&run.status),
        "caller": caller_reply(device, run),
        "startedAt": run.started_at,
        "endedAt": run.ended_at.filter(|end| *end != 0),
        "durationMs": duration,
        "summary": summary,
        "error": run.error.as_deref().map(|e| cap_text(e, MAX_TEXT_BYTES)),
        "nodes": nodes,
        "sessions": sessions,
    });
    if let Some(fields) = value.as_object_mut() {
        fields.retain(|key, _| key == "sessions" || RUN_KEYS.contains(&key.as_str()));
    }
    value
}

fn node_reply(
    run: &WorkflowRun,
    node: &NodeRun,
    links: &Links,
    sessions: &mut Vec<String>,
) -> Value {
    // The serialized record, from which only the allowed keys are taken.
    let mut stored = serde_json::to_value(node).unwrap_or(Value::Null);
    let mut fields = serde_json::Map::new();
    if let Some(stored) = stored.as_object_mut() {
        for key in NODE_KEYS {
            if let Some(value) = stored.remove(key) {
                fields.insert(key.to_owned(), value);
            }
        }
    }
    for key in ["nodeId", "branch"] {
        if let Some(value) = fields.get_mut(key) {
            cap_strings(value, MAX_LABEL_BYTES);
        }
    }
    if let Some(error) = fields.get_mut("error") {
        cap_strings(error, MAX_TEXT_BYTES);
    }
    if let Some(decision) = fields.get_mut("decision") {
        let value = decision.get("value").cloned().unwrap_or(Value::Null);
        let reason = decision.get("reason").cloned();
        let mut clean = json!({"value": value});
        if let Some(reason) = reason {
            clean["reason"] = reason;
        }
        cap_strings(&mut clean["value"], MAX_LABEL_BYTES);
        cap_strings(&mut clean, MAX_TEXT_BYTES);
        *decision = clean;
    }
    if let Some(output) = fields.get_mut("output") {
        if let Some(map) = output.as_object_mut() {
            map.remove("worktree");
            // The session the node ran in, under the phone's id, or not at all.
            if let Some(persisted) = map.remove("sessionId")
                && let Some(id) = persisted.as_str().and_then(|p| link_of(links, p))
            {
                map.insert("sessionId".into(), json!(id));
            }
        }
        cap_strings(output, MAX_TEXT_BYTES);
    }
    let definition = run.definition.node(&node.node_id);
    fields.insert(
        "title".into(),
        definition
            .and_then(|n| n.title.as_deref())
            .map_or(Value::Null, |t| capped(t, MAX_LABEL_BYTES)),
    );
    fields.insert("type".into(), json!(definition.map(|n| n.kind.type_name())));
    let mut linked: Vec<String> = Vec::new();
    let rounds = node
        .history
        .iter()
        .flatten()
        .filter_map(|r| r.session_id.clone());
    for persisted in rounds.chain(node.session_id.clone()) {
        if let Some(id) = link_of(links, &persisted) {
            push_unique(&mut linked, id, MAX_NODE_SESSIONS);
        }
    }
    for id in &linked {
        push_unique(sessions, id.clone(), MAX_RUN_SESSIONS);
    }
    fields.insert("sessions".into(), json!(linked));
    Value::Object(fields)
}

/// Who started the run. A Mono the device did not select is not named.
fn caller_reply(device: &DeviceContext, run: &WorkflowRun) -> Value {
    let stored = serde_json::to_value(&run.caller).unwrap_or(Value::Null);
    let mut caller = serde_json::Map::new();
    for key in CALLER_KEYS {
        let Some(value) = stored.get(key) else {
            continue;
        };
        if key == "monoId"
            && !value
                .as_str()
                .is_some_and(|mono| in_selection(&device.monos, mono))
        {
            continue;
        }
        let mut value = value.clone();
        cap_strings(&mut value, MAX_LABEL_BYTES);
        caller.insert(key.to_owned(), value);
    }
    if let Some(item) = stored.get("item").and_then(Value::as_object) {
        let mut kept = serde_json::Map::new();
        for key in ITEM_KEYS {
            if let Some(value) = item.get(key) {
                let mut value = value.clone();
                cap_strings(&mut value, MAX_LABEL_BYTES);
                kept.insert(key.to_owned(), value);
            }
        }
        caller.insert("item".into(), Value::Object(kept));
    }
    Value::Object(caller)
}

impl Shell {
    pub(super) fn mobile_workflows(&mut self, request: Request, cx: &mut Context<Self>) {
        let Some(dir) = self.workflows.as_ref().map(|w| w.read(cx).dir().clone()) else {
            let empty = if request.method == "workflows.list" {
                json!({"workflows": []})
            } else {
                json!({"workflow": null, "runs": []})
            };
            return request.reply(Ok(empty));
        };
        let runs_of = if request.method == "workflows.runs" {
            match string(&request.params, "workflowId") {
                Some(id) => Some(id),
                None => return request.reply(Err("workflowId is required".into())),
            }
        } else {
            None
        };
        let device = request.device.clone();
        let known = self.mobile_projects(cx);
        let reading = cx.background_executor().spawn({
            let device = device.clone();
            async move { read_files(&dir, runs_of.as_deref(), &device, &known) }
        });
        cx.spawn(async move |this, cx| {
            let loaded = match reading.await {
                Ok(loaded) => loaded,
                Err(error) => return request.reply(Err(error.into())),
            };
            // The one step that needs the shell: which sessions the device may open.
            let ids = session_ids_of(&loaded);
            let Ok(links) =
                this.update(cx, |this, cx| this.workflow_links(&request.device, ids, cx))
            else {
                return;
            };
            let value = cx
                .background_executor()
                .spawn(async move { reply_of(&device, loaded, &links) })
                .await;
            request.reply(Ok(value));
        })
        .detach();
    }

    /// The phone's id for each session a run names, when the device may see that session. Each
    /// persisted id is resolved once.
    pub(super) fn workflow_links(
        &mut self,
        device: &DeviceContext,
        persisted: Vec<String>,
        cx: &App,
    ) -> Links {
        persisted
            .into_iter()
            .map(|persisted| {
                let link = match self.mobile_target(&persisted, device, cx) {
                    Ok(Target::Session(id)) => Some(self.mobile_session_id(id)),
                    _ => None,
                };
                (persisted, link)
            })
            .collect()
    }
}

/// Reads what `device` may see. A workflow it may not see, and one that does not exist, are
/// answered the same and their runs are never read. Errors carry no path.
pub(super) fn read_files(
    dir: &WorkflowsDir,
    runs_of: Option<&str>,
    device: &DeviceContext,
    known: &[String],
) -> Result<Loaded, &'static str> {
    let loaded = load_workflows(dir).map_err(|_| "The workflows could not be read")?;
    let visible = |w: &Workflow| workflow_visible(device, known, w);
    // The run's own snapshot may use other projects than today's definition.
    let run_visible = |run: &WorkflowRun| visible(&with_run_values(&run.definition));
    match runs_of {
        None => Ok(Loaded::List(
            loaded
                .workflows
                .into_iter()
                .filter(|w| visible(w))
                .map(|w| {
                    let last = last_run(dir, device, &w.id, &run_visible);
                    (w, last)
                })
                .collect(),
        )),
        Some(id) => {
            let Some(workflow) = loaded
                .workflows
                .into_iter()
                .find(|w| w.id == id && visible(w))
            else {
                return Ok(Loaded::Runs(None));
            };
            let runs = load_runs_checked(dir, id)
                .map_err(|_| "The runs could not be read")?
                .into_iter()
                .filter(|run| run_visible(run))
                .collect();
            Ok(Loaded::Runs(Some((workflow, runs))))
        }
    }
}

/// The newest run the device may see. A device that sees every project needs only the first.
fn last_run(
    dir: &WorkflowsDir,
    device: &DeviceContext,
    id: &str,
    run_visible: &impl Fn(&WorkflowRun) -> bool,
) -> LastRun {
    if device.projects.is_none() {
        return match last_run_of(dir, id) {
            Ok(Some((status, started))) => LastRun::Seen(status_of(&status), started),
            Ok(None) => LastRun::Never,
            Err(_) => LastRun::Unreadable,
        };
    }
    match load_runs_checked(dir, id) {
        Ok(runs) => runs
            .iter()
            .find(|run| run_visible(run))
            .map_or(LastRun::Never, |run| {
                LastRun::Seen(status_of(&run.status), run.started_at)
            }),
        Err(_) => LastRun::Unreadable,
    }
}
