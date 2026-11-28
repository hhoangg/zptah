//! `workflows.list` and `workflows.runs`: the read-only Workflows screen of the web app.
//!
//! Both are plain reads over the global workflow store. A device that sees only some projects
//! sees a workflow only when it sees every project the workflow uses; a project that cannot be
//! worked out from the definition hides the workflow (fail closed). A run's sessions are shown
//! only where the ordinary session rules (`mobile_target`) allow them, under the phone's session
//! ids. Nothing here starts, changes or stops a workflow.
//!
//! Runs and nodes are passed through as loose objects (the serialized run records), so a status
//! or a field a later desktop adds reaches the web without a protocol change.
use super::*;
use monocode_core::mono::workflows::{
    cli::{has_unresolved_project, workflow_projects},
    global::WorkflowsDir,
    model::{NodeKind, NodeRun, Workflow, WorkflowRun, with_run_values},
    store::{last_run_of, load_runs, load_workflows},
};

/// What the files held, read off the UI thread. Built once per request and consumed at once.
#[allow(clippy::large_enum_variant)]
pub(super) enum Loaded {
    List(Vec<(Workflow, Option<(Value, u64)>)>),
    Runs(Option<Workflow>, Vec<WorkflowRun>),
}

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
const TRUNCATED: &str = "... [truncated]";
/// The most a `workflows.runs` reply may weigh, far under the link's 8 MiB response limit.
const MAX_REPLY_BYTES: usize = 1024 * 1024;

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

/// Every string inside `value` cut to `max` bytes.
fn cap_strings(value: &mut Value, max: usize) {
    match value {
        Value::String(text) if text.len() > max => *text = cap_text(text, max),
        Value::Array(items) => items.iter_mut().for_each(|item| cap_strings(item, max)),
        Value::Object(fields) => fields.values_mut().for_each(|item| cap_strings(item, max)),
        _ => {}
    }
}

/// Cuts the long values of a run and, when the whole reply is still over `budget`, drops the node
/// outputs of the oldest runs first (the list is newest first), marking each with `outputsDropped`.
pub(super) fn bound_runs(runs: &mut [Value], budget: usize) {
    for run in runs.iter_mut() {
        if let Some(text) = run.pointer_mut("/summary/text") {
            cap_strings(text, MAX_SUMMARY_BYTES);
        }
        if let Some(error) = run.get_mut("error") {
            cap_strings(error, MAX_TEXT_BYTES);
        }
        for node in run["nodes"].as_array_mut().into_iter().flatten() {
            for key in ["output", "error"] {
                if let Some(value) = node.get_mut(key) {
                    cap_strings(value, MAX_TEXT_BYTES);
                }
            }
        }
    }
    let size = |runs: &[Value]| serde_json::to_vec(runs).map_or(usize::MAX, |bytes| bytes.len());
    for at in (0..runs.len()).rev() {
        if size(runs) <= budget {
            break;
        }
        for node in runs[at]["nodes"].as_array_mut().into_iter().flatten() {
            if let Some(fields) = node.as_object_mut() {
                fields.remove("output");
            }
        }
        runs[at]["outputsDropped"] = json!(true);
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
            json!({"id": node.id, "type": node.kind.type_name(), "label": label})
        })
        .collect()
}

fn status_of<T: serde::Serialize>(status: &T) -> Value {
    serde_json::to_value(status).unwrap_or(Value::Null)
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
        let work = cx
            .background_executor()
            .spawn(async move { read_files(&dir, runs_of.as_deref()) });
        cx.spawn(async move |this, cx| {
            let loaded = work.await;
            let _ = this.update(cx, |this, cx| {
                let reply = match loaded {
                    Ok(loaded) => Ok(this.workflows_reply(&request.device, loaded, cx)),
                    Err(error) => Err(Failure::from(error)),
                };
                request.reply(reply);
            });
        })
        .detach();
    }

    pub(super) fn workflows_reply(
        &mut self,
        device: &DeviceContext,
        loaded: Loaded,
        cx: &App,
    ) -> Value {
        let known = self.mobile_projects(cx);
        match loaded {
            Loaded::List(workflows) => {
                let list: Vec<Value> = workflows
                    .iter()
                    .filter(|(w, _)| workflow_visible(device, &known, w))
                    .map(|(w, last)| {
                        json!({
                            "id": w.id,
                            "name": w.name,
                            "description": w.description,
                            "enabled": w.enabled,
                            "needsReview": w.needs_review,
                            "triggers": triggers_of(w),
                            "nextRunAt": w.next_run_at,
                            "lastRun": last.as_ref().map(|(status, started)| {
                                json!({"status": status, "startedAt": started})
                            }),
                        })
                    })
                    .collect();
                json!({"workflows": list})
            }
            Loaded::Runs(workflow, runs) => {
                // Hidden and missing look the same.
                let Some(workflow) = workflow.filter(|w| workflow_visible(device, &known, w))
                else {
                    return json!({"workflow": null, "runs": []});
                };
                let runs: Vec<Value> = runs
                    .iter()
                    // The run's own snapshot may use other projects than today's definition.
                    .filter(|run| {
                        workflow_visible(device, &known, &with_run_values(&run.definition))
                    })
                    .map(|run| self.run_reply(device, run, cx))
                    .collect();
                let mut runs = runs;
                bound_runs(&mut runs, MAX_REPLY_BYTES);
                json!({
                    "workflow": {"id": workflow.id, "name": workflow.name},
                    "runs": runs,
                })
            }
        }
    }

    /// The phone's id for a session a run names, when the device may see that session.
    fn run_session_link(
        &mut self,
        device: &DeviceContext,
        persisted: &str,
        cx: &App,
    ) -> Option<String> {
        match self.mobile_target(persisted, device, cx) {
            Ok(Target::Session(id)) => Some(self.mobile_session_id(id)),
            _ => None,
        }
    }

    fn run_reply(&mut self, device: &DeviceContext, run: &WorkflowRun, cx: &App) -> Value {
        let mut sessions: Vec<String> = Vec::new();
        let nodes: Vec<Value> = run
            .nodes
            .iter()
            .map(|node| self.node_reply(device, run, node, &mut sessions, cx))
            .collect();
        let duration = run
            .ended_at
            .filter(|end| *end != 0)
            .map(|end| end.saturating_sub(run.started_at));
        let mut value = json!({
            "id": run.id,
            "status": status_of(&run.status),
            "caller": caller_reply(device, run),
            "startedAt": run.started_at,
            "endedAt": run.ended_at.filter(|end| *end != 0),
            "durationMs": duration,
            "summary": run.summary.as_ref().map(|s| json!({"title": s.title, "text": s.text})),
            "error": run.error,
            "nodes": nodes,
        });
        sessions.dedup();
        value["sessions"] = json!(sessions);
        value
    }

    fn node_reply(
        &mut self,
        device: &DeviceContext,
        run: &WorkflowRun,
        node: &NodeRun,
        sessions: &mut Vec<String>,
        cx: &App,
    ) -> Value {
        // The serialized record, so a field a later desktop adds is passed on as it is. What
        // names a session is replaced below by what the device may see.
        let mut value = serde_json::to_value(node).unwrap_or(Value::Null);
        let Some(fields) = value.as_object_mut() else {
            return Value::Null;
        };
        for key in [
            "sessionId",
            "reuseSessionId",
            "interruptedSessions",
            "history",
        ] {
            fields.remove(key);
        }
        let definition = run.definition.node(&node.node_id);
        fields.insert(
            "title".into(),
            json!(definition.and_then(|n| n.title.clone())),
        );
        fields.insert("type".into(), json!(definition.map(|n| n.kind.type_name())));
        let mut linked: Vec<String> = Vec::new();
        let rounds = node
            .history
            .iter()
            .flatten()
            .filter_map(|r| r.session_id.clone());
        for persisted in rounds.chain(node.session_id.clone()) {
            if let Some(id) = self.run_session_link(device, &persisted, cx)
                && !linked.contains(&id)
            {
                linked.push(id);
            }
        }
        sessions.extend(linked.iter().cloned());
        fields.insert("sessions".into(), json!(linked));
        value
    }
}

/// Who started the run. A Mono the device did not select is not named.
fn caller_reply(device: &DeviceContext, run: &WorkflowRun) -> Value {
    let mut caller = serde_json::to_value(&run.caller).unwrap_or(Value::Null);
    if let Some(fields) = caller.as_object_mut() {
        let hidden = fields
            .get("monoId")
            .and_then(Value::as_str)
            .is_some_and(|mono| !in_selection(&device.monos, mono));
        if hidden {
            fields.remove("monoId");
        }
    }
    caller
}

fn read_files(dir: &WorkflowsDir, runs_of: Option<&str>) -> Result<Loaded, String> {
    let loaded = load_workflows(dir)?;
    match runs_of {
        None => Ok(Loaded::List(
            loaded
                .workflows
                .into_iter()
                .map(|w| {
                    let last = last_run_of(dir, &w.id)
                        .ok()
                        .flatten()
                        .map(|(status, started)| (status_of(&status), started));
                    (w, last)
                })
                .collect(),
        )),
        Some(id) => {
            let workflow = loaded.workflows.into_iter().find(|w| w.id == id);
            let runs = match workflow {
                Some(_) => load_runs(dir, id)?,
                None => Vec::new(),
            };
            Ok(Loaded::Runs(workflow, runs))
        }
    }
}
