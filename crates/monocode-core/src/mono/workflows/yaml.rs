//! Sharing a workflow as a file: YAML (or JSON) export, and the import a user reviews before
//! anything runs. Port of the React app's `workflowYaml.ts`; a file written by either app reads
//! in the other.
//!
//! The file holds name, description, input declarations (never their values), nodes with board
//! positions and edges. Every string in it is checked for absolute paths, so a file can be
//! handed to someone else.
use super::model::{
    new_workflow_id, Checkout, Concurrency, Cwd, DecisionModel, Edge, InputValue, Node, NodeId,
    NodeKind, OutputField, OutputFieldType, Requirement, SessionTarget, TargetInstance, Workflow,
    WorkflowId, WorkflowInput, WorkflowUi, Workspace, DECIDE_AI, DIRECTION_LR, NODE_TYPES,
};
use super::models::{workflow_issues, ModelCatalog};
use super::ordered::{Json, OrderedMap};
use super::output::output_hint;
use super::parse::{
    drop_stray_handles, parse_concurrency, parse_decision_model, parse_edge, parse_inputs,
    parse_node, parse_workflow_ui, parse_workspace, read_requires,
};
use super::template::{js_space, js_trim};
use super::validate::{find_absolute_path, WorkflowIssue};
use super::yaml_emit::{stringify, Doc};
use super::yaml_read::{parse_yaml, ReadError};
use crate::mono::habits::Schedule;

/// The value of the top-level `monoptahWorkflow` key.
pub const WORKFLOW_FILE_VERSION: u32 = 1;
/// Larger files are refused before they are parsed.
pub const MAX_FILE_BYTES: usize = 512 * 1024;
const NESTING_MAX: usize = super::yaml_read::NESTING_MAX;
const TOO_DEEP: &str = "The workflow file is nested too deeply";

// --- export --------------------------------------------------------------------------------

fn text(s: &str) -> Doc {
    Doc::Str(s.to_string())
}

/// Appends `key: value` when there is a value, like `defined({...})` drops what is undefined.
fn put(entries: &mut Vec<(String, Doc)>, key: &str, value: Option<Doc>) {
    if let Some(value) = value {
        entries.push((key.to_string(), value));
    }
}

fn number(n: impl Into<f64>) -> Doc {
    Doc::Num(n.into())
}

/// The file writes a schedule the way the spec does, `{ kind, time, ... }`; fields a schedule
/// keeps at their defaults are left out. The time is always quoted, because a YAML 1.1 reader
/// takes a bare 09:00 for a number.
pub(crate) fn export_schedule(s: &Schedule) -> Doc {
    let mut out = vec![
        ("kind".to_string(), text(&s.schedule_kind)),
        ("time".to_string(), Doc::Quoted(s.time.clone())),
    ];
    put(
        &mut out,
        "minute",
        (s.minute != 0).then(|| number(s.minute)),
    );
    put(
        &mut out,
        "dayOfWeek",
        (s.day_of_week != 1).then(|| number(s.day_of_week)),
    );
    put(&mut out, "hours", s.interval_hours.map(number));
    put(
        &mut out,
        "intervalFrom",
        s.interval_from.as_deref().map(text),
    );
    Doc::Map(out)
}

fn export_output(output: &OrderedMap<OutputField>) -> Doc {
    Doc::Map(
        output
            .iter()
            .map(|(name, f)| {
                let ty = OutputFieldType::ALL
                    .iter()
                    .find(|(_, t)| *t == f.ty)
                    .map_or("string", |(n, _)| n);
                let mut field = vec![("type".to_string(), text(ty))];
                put(&mut field, "inReport", f.in_report.map(Doc::Bool));
                put(
                    &mut field,
                    "description",
                    f.description.as_deref().map(text),
                );
                (name.clone(), Doc::Map(field))
            })
            .collect(),
    )
}

fn worktree_of(id: &str) -> Doc {
    Doc::Map(vec![("worktreeOf".to_string(), text(id))])
}

fn doc_of(value: &Json) -> Doc {
    match value {
        Json::Null => Doc::Null,
        Json::Bool(b) => Doc::Bool(*b),
        Json::Number(n) => Doc::Num(*n),
        Json::String(s) => text(s),
        Json::Array(items) => Doc::Seq(items.iter().map(doc_of).collect()),
        Json::Object(map) => Doc::Map(map.iter().map(|(k, v)| (k.clone(), doc_of(v))).collect()),
    }
}

fn export_target(target: &SessionTarget) -> Doc {
    let mut out = vec![("kind".to_string(), text(&target.kind))];
    match &target.instance {
        TargetInstance::New => {}
        TargetInstance::SameAs(id) => out.push((
            "instance".to_string(),
            Doc::Map(vec![("sameAs".to_string(), text(id))]),
        )),
        TargetInstance::Invalid(raw) => out.push(("instance".to_string(), doc_of(raw))),
    }
    for (key, value) in target.options.iter() {
        out.push((key.clone(), doc_of(value)));
    }
    Doc::Map(out)
}

fn export_node(node: &Node) -> Doc {
    let mut out = vec![
        ("id".to_string(), text(&node.id)),
        ("type".to_string(), text(node.kind.type_name())),
    ];
    put(&mut out, "title", node.title.as_deref().map(text));
    put(&mut out, "parent", node.parent.as_deref().map(text));
    let ui = node.ui.map(|ui| {
        Doc::Map(vec![
            ("x".to_string(), number(ui.x)),
            ("y".to_string(), number(ui.y)),
        ])
    });
    match &node.kind {
        NodeKind::Schedule { schedule } => {
            put(&mut out, "schedule", Some(export_schedule(schedule)));
        }
        NodeKind::Manual => {}
        NodeKind::Session {
            project,
            prompt,
            checkout,
            output,
            mode,
            surface,
            harness,
            model,
            effort,
            reuse_session,
            nest_under,
            timeout_minutes,
            target,
            account,
        } => {
            put(&mut out, "project", (!project.is_empty()).then(|| text(project)));
            put(&mut out, "harness", harness.as_deref().map(text));
            put(&mut out, "model", model.as_deref().map(text));
            put(&mut out, "effort", effort.as_deref().map(text));
            put(&mut out, "surface", surface.as_deref().map(text));
            put(&mut out, "mode", mode.as_deref().map(text));
            put(
                &mut out,
                "checkout",
                Some(match checkout {
                    Checkout::Run => text("run"),
                    Checkout::Root => text("root"),
                    Checkout::Existing(path) => {
                        Doc::Map(vec![("existing".to_string(), text(path))])
                    }
                    Checkout::NewWorktree => text("newWorktree"),
                    Checkout::WorktreeOf(id) => worktree_of(id),
                }),
            );
            put(&mut out, "nestUnder", nest_under.as_deref().map(text));
            put(&mut out, "reuseSession", reuse_session.map(Doc::Bool));
            put(&mut out, "timeoutMinutes", timeout_minutes.map(number));
            put(&mut out, "target", target.as_ref().map(export_target));
            put(&mut out, "account", account.as_deref().map(text));
            put(&mut out, "prompt", Some(text(prompt)));
            put(&mut out, "outputs", Some(export_output(output)));
        }
        NodeKind::Command {
            project,
            command,
            cwd,
            timeout_minutes,
            on_failure,
        } => {
            put(&mut out, "project", (!project.is_empty()).then(|| text(project)));
            put(&mut out, "onFailure", on_failure.as_deref().map(text));
            put(
                &mut out,
                "cwd",
                cwd.as_ref().map(|cwd| match cwd {
                    Cwd::Root => text("root"),
                    Cwd::WorktreeOf(id) => worktree_of(id),
                }),
            );
            put(&mut out, "timeoutMinutes", timeout_minutes.map(number));
            put(&mut out, "command", Some(text(command)));
        }
        NodeKind::If {
            condition,
            decide,
            question,
        } => {
            put(&mut out, "condition", condition.as_deref().map(text));
            put(&mut out, "decide", decide.map(|_| text(DECIDE_AI)));
            put(&mut out, "question", question.as_deref().map(text));
        }
        NodeKind::Switch {
            value,
            cases,
            decide,
            question,
        } => {
            put(&mut out, "value", value.as_deref().map(text));
            put(
                &mut out,
                "cases",
                Some(Doc::Seq(cases.iter().map(|c| text(c)).collect())),
            );
            put(&mut out, "decide", decide.map(|_| text(DECIDE_AI)));
            put(&mut out, "question", question.as_deref().map(text));
        }
        NodeKind::ForEach {
            list,
            item,
            max_parallel,
        } => {
            put(&mut out, "in", Some(text(list)));
            put(&mut out, "as", item.as_deref().map(text));
            put(&mut out, "maxParallel", max_parallel.map(number));
        }
    }
    put(&mut out, "ui", ui);
    Doc::Map(out)
}

fn export_edge(edge: &Edge) -> Doc {
    let mut out = vec![
        ("from".to_string(), text(&edge.from)),
        ("to".to_string(), text(&edge.to)),
    ];
    put(
        &mut out,
        "handle",
        edge.handle.as_ref().map(|h| text(h.as_str())),
    );
    put(
        &mut out,
        "loop",
        edge.loop_spec.as_ref().map(|l| {
            Doc::Map(vec![
                ("maxRounds".to_string(), number(l.max_rounds)),
                ("decide".to_string(), text(&l.decide)),
            ])
        }),
    );
    Doc::Map(out)
}

fn export_input(input: &WorkflowInput) -> Doc {
    let ty = super::model::InputType::ALL
        .iter()
        .find(|(_, t)| *t == input.ty)
        .map_or("string", |(n, _)| n);
    let mut out = vec![("type".to_string(), text(ty))];
    put(
        &mut out,
        "description",
        input.description.as_deref().map(text),
    );
    Doc::Map(out)
}

fn export_requirement(r: &Requirement) -> Doc {
    let mut out = vec![(r.kind.key().to_string(), text(&r.name))];
    put(&mut out, "hint", r.hint.as_deref().map(text));
    Doc::Map(out)
}

fn export_workspace(workspace: &Workspace) -> Doc {
    let mut out = vec![("mode".to_string(), text(workspace.mode.name()))];
    put(
        &mut out,
        "project",
        (!workspace.project.is_empty()).then(|| text(&workspace.project)),
    );
    put(
        &mut out,
        "baseBranch",
        workspace.base_branch.as_deref().map(text),
    );
    Doc::Map(out)
}

fn export_decision_model(model: &DecisionModel) -> Doc {
    let mut out = vec![("harness".to_string(), text(&model.harness))];
    put(&mut out, "model", model.model.as_deref().map(text));
    put(&mut out, "effort", model.effort.as_deref().map(text));
    Doc::Map(out)
}

/// The shareable definition: only name, description, inputs (declarations, no values),
/// requirements, the decision model and summary template, nodes (with board positions) and
/// edges. `reportTo` names a Mono of this computer, so it is not shared. The output is
/// deterministic.
fn export_document(w: &Workflow) -> Doc {
    let mut out = vec![
        (
            "monoptahWorkflow".to_string(),
            Doc::Num(WORKFLOW_FILE_VERSION.into()),
        ),
        ("name".to_string(), text(&w.name)),
    ];
    put(&mut out, "description", w.description.as_deref().map(text));
    out.push((
        "inputs".to_string(),
        Doc::Map(
            w.inputs
                .iter()
                .map(|(k, v)| (k.clone(), export_input(v)))
                .collect(),
        ),
    ));
    if !w.requires.is_empty() {
        out.push((
            "requires".to_string(),
            Doc::Seq(w.requires.iter().map(export_requirement).collect()),
        ));
    }
    put(
        &mut out,
        "decisionModel",
        w.decision_model.as_ref().map(export_decision_model),
    );
    put(
        &mut out,
        "summaryTemplate",
        w.summary_template
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(text),
    );
    if w.is_lr() {
        out.push((
            "ui".to_string(),
            Doc::Map(vec![("direction".to_string(), text(DIRECTION_LR))]),
        ));
    }
    put(
        &mut out,
        "workspace",
        (!w.workspace.is_default()).then(|| export_workspace(&w.workspace)),
    );
    put(
        &mut out,
        "concurrency",
        (!w.concurrency.is_default())
            .then(|| Doc::Map(vec![("max".to_string(), number(w.concurrency.max))])),
    );
    out.push((
        "nodes".to_string(),
        Doc::Seq(w.nodes.iter().map(export_node).collect()),
    ));
    out.push((
        "edges".to_string(),
        Doc::Seq(w.edges.iter().map(export_edge).collect()),
    ));
    Doc::Map(out)
}

/// The workflow as YAML text. Long prompts stay on one line instead of being folded.
pub fn export_workflow(w: &Workflow) -> String {
    stringify(&export_document(w))
}

/// Why this workflow cannot be shared, when a string in it (name, description, input
/// descriptions, prompts, commands...) reads as an absolute path: the same rule `preview_import`
/// applies, so a file the app writes is a file it can read back. None when it is fine.
pub fn export_path_problem(w: &Workflow) -> Option<String> {
    let problem = find_path_field(&export_document(w).to_json(), "", 0)?;
    if problem == TOO_DEEP {
        return Some(problem);
    }
    Some(format!("{problem}; use a project input or a relative path"))
}

// --- import --------------------------------------------------------------------------------

/// The definition a file holds, before it gets an id, a creation time and input values.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDraft {
    pub name: String,
    pub description: Option<String>,
    pub inputs: OrderedMap<WorkflowInput>,
    pub requires: Vec<Requirement>,
    pub decision_model: Option<DecisionModel>,
    pub summary_template: Option<String>,
    /// The board layout marker (`ui.direction`), so a shared file opens laid out the same way.
    pub ui: Option<WorkflowUi>,
    pub workspace: Workspace,
    pub concurrency: Concurrency,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl From<&Workflow> for WorkflowDraft {
    fn from(w: &Workflow) -> Self {
        Self {
            name: w.name.clone(),
            description: w.description.clone(),
            inputs: w.inputs.clone(),
            requires: w.requires.clone(),
            decision_model: w.decision_model.clone(),
            summary_template: w.summary_template.clone(),
            ui: w.ui.clone(),
            workspace: w.workspace.clone(),
            concurrency: w.concurrency,
            nodes: w.nodes.clone(),
            edges: w.edges.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewCommand {
    pub node_id: NodeId,
    pub command: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewPrompt {
    pub node_id: NodeId,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImportPreview {
    pub workflow: WorkflowDraft,
    /// Every command node's command, verbatim, for the user to review.
    pub commands: Vec<ReviewCommand>,
    /// Every text that reaches an agent, as the agent will read it before templates are filled
    /// in: a session's prompt followed by the output instruction (which carries the output
    /// field descriptions), the questions put to the AI, and the summary template (the summary
    /// can be posted into a Mono's chat, so that Mono reads it as its own reply).
    pub prompts: Vec<ReviewPrompt>,
    pub issues: Vec<WorkflowIssue>,
    /// What the import changed to fit this app (a Report node folded into `summaryTemplate`).
    pub warnings: Vec<String>,
}

/// What a user reviews before a workflow runs: every command verbatim, and every text that
/// reaches an agent as the agent will read it before templates are filled in.
pub fn review_texts(nodes: &[Node]) -> (Vec<ReviewCommand>, Vec<ReviewPrompt>) {
    let mut commands = Vec::new();
    let mut prompts = Vec::new();
    let mut prompt = |node: &Node, text: String| {
        prompts.push(ReviewPrompt {
            node_id: node.id.clone(),
            text,
        })
    };
    for node in nodes {
        match &node.kind {
            NodeKind::Command { command, .. } => commands.push(ReviewCommand {
                node_id: node.id.clone(),
                command: command.clone(),
            }),
            NodeKind::Session {
                prompt: p, output, ..
            } => {
                // The hint the app adds to the first prompt, with the app's command written by
                // its name (a session is given the app's own path there).
                let hint = output_hint(output, crate::brand::NATIVE_PRODUCT);
                prompt(
                    node,
                    if hint.is_empty() {
                        p.clone()
                    } else {
                        format!("{p}\n\n{hint}")
                    },
                );
            }
            NodeKind::If {
                question: Some(question),
                ..
            }
            | NodeKind::Switch {
                question: Some(question),
                ..
            } => prompt(node, question.clone()),
            _ => {}
        }
    }
    (commands, prompts)
}

/// The id a summary template is listed under in the review (it has no node).
pub const SUMMARY_REVIEW_ID: &str = "summary";

/// The summary template as a review item, when there is one that is not blank.
pub fn summary_review_prompt(template: Option<&str>) -> Option<ReviewPrompt> {
    template
        .filter(|t| !js_trim(t).is_empty())
        .map(|t| ReviewPrompt {
            node_id: SUMMARY_REVIEW_ID.to_string(),
            text: t.to_string(),
        })
}

/// The review of a workflow already stored, built from its current definition.
pub fn review_of(w: &Workflow, catalog: &ModelCatalog) -> ImportPreview {
    let (commands, mut prompts) = review_texts(&w.nodes);
    prompts.extend(summary_review_prompt(w.summary_template.as_deref()));
    ImportPreview {
        workflow: WorkflowDraft::from(w),
        commands,
        prompts,
        issues: workflow_issues(w, catalog),
        warnings: Vec::new(),
    }
}

/// The parsed file, or the message to show when it cannot be read.
fn parse_file(text: &str) -> Result<Json, String> {
    parse_file_as(text, "workflow")
}

/// `parse_file` for any shared file; `what` names it in the messages ("workflow", "habit").
pub(crate) fn parse_file_as(text: &str, what: &str) -> Result<Json, String> {
    if text.len() > MAX_FILE_BYTES {
        return fail(format!(
            "{} file is too large (over 512 KiB)",
            capitalized(what)
        ));
    }
    if js_trim(text).is_empty() {
        return fail(format!("{} file is empty", capitalized(what)));
    }
    if text.trim_start_matches(js_space).starts_with('{') {
        if let Some(value) = Json::parse(text) {
            return Ok(value);
        }
        // Not JSON after all; the YAML reader reports what is wrong.
    }
    parse_yaml(text).map_err(|err| match err {
        ReadError::TooDeep => format!("The {what} file is nested too deeply"),
        ReadError::Message(m) => format!(
            "Could not read the {what} file: {}",
            m.lines().next().unwrap_or("")
        ),
    })
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The first string anywhere in `value` that reads as an absolute path, with where it sits.
/// Nesting past 64 levels is reported as `TOO_DEEP` rather than followed.
fn find_path_field(value: &Json, label: &str, depth: usize) -> Option<String> {
    if depth > NESTING_MAX {
        return Some(TOO_DEEP.to_string());
    }
    match value {
        Json::String(s) => find_absolute_path(s)
            .map(|found| format!("{label} contains an absolute path (\"{found}\")")),
        Json::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(i, item)| find_path_field(item, &format!("{label}[{i}]"), depth + 1)),
        Json::Object(map) => map.iter().find_map(|(key, item)| {
            let at = if label.is_empty() {
                key.clone()
            } else {
                format!("{label}.{key}")
            };
            find_path_field(item, &at, depth + 1)
        }),
        _ => None,
    }
}

pub(crate) fn describe_version(v: Option<&Json>) -> String {
    match v {
        Some(Json::Number(n)) => super::ordered::js_number_text(*n),
        Some(Json::String(s)) => s.clone(),
        _ => "missing".to_string(),
    }
}

/// `{ ...schedule, scheduleKind: kind, intervalHours: hours ?? intervalHours }`: the file's
/// friendly `kind`/`hours` become the stored names. A schedule that already says `scheduleKind`
/// or has no `kind` is left as it is.
fn import_schedule(raw: Option<&Json>) -> Option<Json> {
    let raw = raw?;
    let map = raw.as_object()?;
    if map.contains_key("scheduleKind") || !map.contains_key("kind") {
        return Some(raw.clone());
    }
    let mut out = OrderedMap::new();
    for (key, value) in map.iter() {
        if key != "kind" && key != "hours" {
            out.insert(key.clone(), value.clone());
        }
    }
    out.insert(
        "scheduleKind",
        map.get("kind").cloned().unwrap_or(Json::Null),
    );
    // `hours ?? intervalHours`: a null counts as missing.
    if let Some(hours) = map.get("hours").filter(|h| **h != Json::Null) {
        out.insert("intervalHours", hours.clone());
    }
    Some(Json::Object(out))
}

fn fail<T>(message: impl Into<String>) -> Result<T, String> {
    Err(message.into())
}

/// `trigger` is reserved for the event item (`{{trigger.item}}`), but a file written before that
/// used it as the id of its trigger node. Such a node gets a free id, and the edges, the `parent`
/// fields and the `{{trigger.*}}` references that meant its outputs follow it.
fn rename_trigger_node(file: &mut OrderedMap<Json>, warnings: &mut Vec<String>) {
    const OLD: &str = "trigger";
    let ids: Vec<String> = match file.get("nodes") {
        Some(Json::Array(nodes)) => nodes
            .iter()
            .filter_map(|n| n.get("id").and_then(Json::as_str).map(str::to_string))
            .collect(),
        _ => return,
    };
    if !ids.iter().any(|id| id == OLD) {
        return;
    }
    let new_id = std::iter::once("start".to_string())
        .chain((2..).map(|n| format!("start{n}")))
        .find(|candidate| !ids.contains(candidate))
        .unwrap_or_default();
    if let Some(Json::Array(nodes)) = file.get_mut("nodes") {
        for node in nodes.iter_mut() {
            rename_in_node(node, OLD, &new_id);
        }
    }
    if let Some(Json::Array(edges)) = file.get_mut("edges") {
        for edge in edges.iter_mut() {
            if let Json::Object(fields) = edge {
                for key in ["from", "to"] {
                    if fields.get(key).and_then(Json::as_str) == Some(OLD) {
                        fields.insert(key, Json::String(new_id.clone()));
                    }
                }
            }
        }
    }
    if let Some(Json::String(template)) = file.get_mut("summaryTemplate") {
        *template = rename_template_head(template, OLD, &new_id);
    }
    warnings.push(format!(
        "The node \"{OLD}\" was renamed \"{new_id}\": \"{OLD}\" now names the event item, and its references were updated"
    ));
}

fn rename_in_node(node: &mut Json, old: &str, new: &str) {
    let Json::Object(fields) = node else {
        return;
    };
    let keys: Vec<String> = fields.keys().cloned().collect();
    for key in keys {
        let Some(value) = fields.get_mut(&key) else {
            continue;
        };
        if key == "id" || key == "parent" {
            if value.as_str() == Some(old) {
                *value = Json::String(new.to_string());
            }
        } else {
            rename_in_templates(value, old, new);
        }
    }
}

fn rename_in_templates(value: &mut Json, old: &str, new: &str) {
    match value {
        Json::String(text) => *text = rename_template_head(text, old, new),
        Json::Array(items) => items
            .iter_mut()
            .for_each(|item| rename_in_templates(item, old, new)),
        Json::Object(fields) => {
            let keys: Vec<String> = fields.keys().cloned().collect();
            for key in keys {
                if let Some(item) = fields.get_mut(&key) {
                    rename_in_templates(item, old, new);
                }
            }
        }
        _ => {}
    }
}

/// `{{old.x}}` and `{{#if old.x}}` become `{{new.x}}` and `{{#if new.x}}`.
pub(super) fn rename_template_head(text: &str, old: &str, new: &str) -> String {
    if !text.contains("{{") {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let old_chars: Vec<char> = old.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' && chars.get(i + 1) == Some(&'{') {
            let mut j = i + 2;
            while j < chars.len() && js_space(chars[j]) {
                j += 1;
            }
            let if_end = j + 3;
            if chars.get(j..if_end).is_some_and(|c| c == ['#', 'i', 'f'])
                && chars.get(if_end).is_some_and(|c| js_space(*c))
            {
                j = if_end;
                while j < chars.len() && js_space(chars[j]) {
                    j += 1;
                }
            }
            let end = j + old_chars.len();
            if chars.get(j..end) == Some(old_chars.as_slice())
                && chars
                    .get(end)
                    .is_none_or(|c| *c == '.' || *c == '}' || js_space(*c))
            {
                out.extend(&chars[i..j]);
                out.push_str(new);
                i = end;
                continue;
            }
            out.extend(&chars[i..j]);
            i = j;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Reads YAML or JSON. Validation problems are reported in `issues`, not as a rejection.
/// `catalog` is what the session nodes' models are judged against.
pub fn preview_import(text: &str, catalog: &ModelCatalog) -> Result<ImportPreview, String> {
    let file = parse_file(text)?;
    let Json::Object(mut file) = file else {
        return fail("Not a zptah workflow file");
    };
    let file_json = Json::Object(file.clone());

    if file.get("monoptahWorkflow") != Some(&Json::Number(WORKFLOW_FILE_VERSION.into())) {
        return fail(format!(
            "Unsupported workflow file version: {} (this app reads version {WORKFLOW_FILE_VERSION})",
            describe_version(file.get("monoptahWorkflow"))
        ));
    }
    if let Some(problem) = find_path_field(&file_json, "", 0) {
        if problem == TOO_DEEP {
            return fail(TOO_DEEP);
        }
        return fail(format!("{problem}; use a project input or a relative path"));
    }

    let name = match file.get("name") {
        Some(Json::String(name)) if !js_trim(name).is_empty() => name.clone(),
        _ => return fail("The workflow file needs a name"),
    };
    let description = match file.get("description") {
        None => None,
        Some(Json::String(d)) => Some(d.clone()),
        Some(_) => return fail("description must be text"),
    };
    if !matches!(file.get("nodes"), Some(Json::Array(_))) {
        return fail("The workflow file needs a list of nodes");
    }
    let mut warnings = Vec::new();
    rename_trigger_node(&mut file, &mut warnings);
    let Some(Json::Array(raw_nodes)) = file.get("nodes") else {
        return fail("The workflow file needs a list of nodes");
    };

    let mut nodes = Vec::new();
    // A Report node of an older file is folded into `summaryTemplate`, and its edges go with it.
    let mut folded_ids: Vec<String> = Vec::new();
    let mut folded_template: Option<String> = None;
    for (index, raw) in raw_nodes.iter().enumerate() {
        let id = raw
            .get("id")
            .and_then(Json::as_str)
            .filter(|id| !id.is_empty());
        let (Json::Object(fields), Some(id)) = (raw, id) else {
            return fail(format!("nodes[{index}] needs an id"));
        };
        let ty = fields.get("type").and_then(Json::as_str);
        if ty == Some("askMono") {
            return fail(format!(
                "Node \"{id}\" is an Ask Mono node, which this app no longer has: use a Switch node with decide \"ai\", with the answers as its cases"
            ));
        }
        if ty == Some("report") {
            folded_ids.push(id.to_string());
            let template = fields
                .get("template")
                .and_then(Json::as_str)
                .filter(|t| !js_trim(t).is_empty());
            let kept = template.is_some() && folded_template.is_none();
            if kept {
                folded_template = template.map(str::to_string);
            }
            warnings.push(format!(
                "The Report node \"{id}\" was removed: every run now ends with a summary{}",
                if kept {
                    ", and its template became the workflow's summaryTemplate"
                } else if template.is_some() {
                    ", and its template was dropped (another one is already used)"
                } else {
                    ""
                }
            ));
            continue;
        }
        if !ty.is_some_and(|t| NODE_TYPES.contains(&t)) {
            let shown = fields
                .get("type")
                .map_or_else(|| "missing".to_string(), |t| t.stringify(false));
            return fail(format!("Node \"{id}\" has an unknown type: {shown}"));
        }
        let prepared = if ty == Some("schedule") {
            let mut with_schedule = fields.clone();
            match import_schedule(fields.get("schedule")) {
                Some(schedule) => with_schedule.insert("schedule", schedule),
                None => {
                    with_schedule.remove("schedule");
                }
            }
            Json::Object(with_schedule)
        } else {
            raw.clone()
        };
        let Some(node) = parse_node(&prepared) else {
            return fail(format!(
                "Node \"{id}\" is missing a required field or has one of the wrong type"
            ));
        };
        nodes.push(node);
    }

    let raw_edges: &[Json] = match file.get("edges") {
        None => &[],
        Some(Json::Array(edges)) => edges,
        Some(_) => return fail("edges must be a list"),
    };
    let mut edges = Vec::new();
    for (index, raw) in raw_edges.iter().enumerate() {
        let Some(edge) = parse_edge(raw) else {
            return fail(format!(
                "edge {} needs \"from\" and \"to\" (and a valid loop, if any)",
                index + 1
            ));
        };
        if !folded_ids.contains(&edge.from) && !folded_ids.contains(&edge.to) {
            edges.push(edge);
        }
    }
    drop_stray_handles(&nodes, &mut edges);

    let requires = read_requires(file.get("requires"))?;
    let Some(decision_model) = parse_decision_model(file.get("decisionModel")) else {
        return fail("decisionModel needs a harness and a model (and optionally an effort)");
    };
    let summary_template = match file.get("summaryTemplate") {
        None | Some(Json::Null) => folded_template,
        Some(Json::String(template)) => {
            if folded_template.is_some() {
                warnings.push(
                    "The Report template was dropped: the file already has a summaryTemplate"
                        .to_string(),
                );
            }
            Some(template.clone())
        }
        Some(_) => return fail("summaryTemplate must be text"),
    };
    let Some(workspace) = parse_workspace(file.get("workspace")) else {
        return fail("workspace needs a mode of perRun or none, and optionally a project and a baseBranch");
    };
    let Some(concurrency) = parse_concurrency(file.get("concurrency")) else {
        return fail("concurrency needs a max, a number from 1 to 10");
    };
    let workflow = WorkflowDraft {
        name,
        description,
        inputs: parse_inputs(file.get("inputs")),
        requires,
        decision_model,
        summary_template,
        ui: parse_workflow_ui(file.get("ui")),
        workspace,
        concurrency,
        nodes,
        edges,
    };
    let mut preview = Workflow::new("wf-preview", workflow.name.clone());
    preview.description = workflow.description.clone();
    preview.inputs = workflow.inputs.clone();
    preview.requires = workflow.requires.clone();
    preview.decision_model = workflow.decision_model.clone();
    preview.summary_template = workflow.summary_template.clone();
    preview.workspace = workflow.workspace.clone();
    preview.concurrency = workflow.concurrency;
    preview.nodes = workflow.nodes.clone();
    preview.edges = workflow.edges.clone();
    let issues = workflow_issues(&preview, catalog);
    let (commands, mut prompts) = review_texts(&workflow.nodes);
    prompts.extend(summary_review_prompt(workflow.summary_template.as_deref()));
    Ok(ImportPreview {
        workflow,
        commands,
        prompts,
        issues,
        warnings,
    })
}

// --- review text ---------------------------------------------------------------------------

/// The text with the characters that change how it reads or what it looks like, without
/// showing, made visible as `<U+XXXX>`: every format character (bidirectional controls,
/// zero-width characters, tag characters, soft hyphens), the line and paragraph separators, a
/// carriage return that is not half of a line break, and the other control characters. What is
/// reviewed is then what runs. Port of `visible()` in `WorkflowReviewSections.tsx`: a character
/// outside the BMP is one marker with its full number.
pub fn visible(text: &str) -> String {
    static FORMAT: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^\p{Cf}$").expect("a valid pattern"));
    let hidden = |c: char, next: Option<char>| {
        if c.is_ascii() {
            return match c {
                '\r' => next != Some('\n'),
                '\n' | '\t' => false,
                _ => c.is_control(),
            };
        }
        c == '\u{2028}'
            || c == '\u{2029}'
            || c.is_control()
            || FORMAT.is_match(c.encode_utf8(&mut [0; 4]))
    };
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if hidden(c, chars.peek().copied()) {
            out.push_str(&format!("<U+{:04X}>", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

// --- values and finishing ------------------------------------------------------------------

/// What is wrong with one input's value, if anything.
fn input_value_problem(
    name: &str,
    input: &WorkflowInput,
    value: Option<&InputValue>,
) -> Option<String> {
    use super::model::InputType;
    let Some(value) = value else {
        return Some(format!("Missing a value for input \"{name}\""));
    };
    match input.ty {
        InputType::Project => match value {
            InputValue::String(s) if !js_trim(s).is_empty() => None,
            _ => Some(format!("Input \"{name}\" needs a project")),
        },
        InputType::String => match value {
            InputValue::String(_) => None,
            _ => Some(format!("Input \"{name}\" must be text")),
        },
        InputType::Number => match value {
            InputValue::Number(n) if n.is_finite() => None,
            _ => Some(format!("Input \"{name}\" must be a number")),
        },
        InputType::Boolean => match value {
            InputValue::Bool(_) => None,
            _ => Some(format!("Input \"{name}\" must be true or false")),
        },
    }
}

/// Every way `values` does not fit the declared `inputs`: a declared input with no value or one
/// of the wrong type (an empty project counts), and a value for an input that is not declared.
/// The one rule behind imports, `workflows.setInputs` and validating a workflow before it is
/// enabled.
pub fn input_value_problems(
    inputs: &OrderedMap<WorkflowInput>,
    values: &OrderedMap<InputValue>,
) -> Vec<String> {
    let mut problems: Vec<String> = inputs
        .iter()
        .filter_map(|(name, input)| input_value_problem(name, input, values.get(name)))
        .collect();
    problems.extend(
        values
            .keys()
            .filter(|name| !inputs.contains_key(name))
            .map(|name| format!("Unknown input \"{name}\"; the workflow declares no such input")),
    );
    problems
}

/// An interval schedule counts from the day it was created or imported.
pub fn anchor_node(mut node: Node, now: u64) -> Node {
    if let NodeKind::Schedule { schedule } = &mut node.kind {
        if schedule.interval_from.as_deref().is_none_or(str::is_empty) {
            *schedule = std::mem::take(schedule).anchor(now);
        }
    }
    node
}

/// The workflow to save after the user reviewed the preview: a new id, paused, with a value for
/// every declared input. The message names the input when a value is missing or has the wrong
/// type. The caller marks it as needing review and records where it came from.
pub fn finish_import(
    preview: &ImportPreview,
    input_values: &OrderedMap<InputValue>,
    now: u64,
) -> Result<Workflow, String> {
    finish_import_with_id(preview, input_values, now, new_workflow_id())
}

/// `finish_import` with the id chosen by the caller.
pub fn finish_import_with_id(
    preview: &ImportPreview,
    input_values: &OrderedMap<InputValue>,
    now: u64,
    id: WorkflowId,
) -> Result<Workflow, String> {
    // Values for inputs the file does not declare are dropped, not an error.
    let mut values = OrderedMap::new();
    for name in preview.workflow.inputs.keys() {
        if let Some(value) = input_values.get(name) {
            values.insert(name.clone(), value.clone());
        }
    }
    if let Some(problem) = input_value_problems(&preview.workflow.inputs, &values)
        .into_iter()
        .next()
    {
        return Err(problem);
    }
    let draft = preview.workflow.clone();
    let mut workflow = Workflow::new(id, draft.name);
    workflow.description = draft.description;
    workflow.inputs = draft.inputs;
    workflow.requires = draft.requires;
    workflow.decision_model = draft.decision_model;
    workflow.summary_template = draft.summary_template;
    workflow.workspace = draft.workspace;
    workflow.concurrency = draft.concurrency;
    workflow.input_values = values;
    workflow.nodes = draft.nodes;
    workflow.edges = draft.edges;
    workflow.ui = draft.ui;
    workflow.created_at = now;
    Ok(workflow)
}
