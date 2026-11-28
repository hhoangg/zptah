//! The board editor's rules with no window in them: adding, connecting, looping, dropping and
//! deleting nodes, what a field of the inspector does to a node, and how a draft is saved over
//! the stored copy. Port of the logic in `WorkflowBoard.tsx` and `NodeInspector.tsx`; the GPUI
//! board draws and calls these.
use super::board_model::{
    auto_place, connection_kind, definition_key, deletion, drop_crossing_edges, drop_nodes,
    fit_groups, from_flow, groupable, is_group, make_edge, new_node, node_height,
    project_input_for, push_right, settle_drop, stack_triggers, to_flow, trigger_frame,
    trigger_frame_size, trigger_slot, BoardNodeData, ConnectionKind, Flow, FlowEdge, FlowKind,
    FlowNode, NodeType, Point, EMPTY_GROUP_H, EMPTY_GROUP_W, GAP_X, NODE_W,
};
use super::engine::looped_node_ids;
use super::model::{
    Checkout, Cwd, Decide, DecisionModel, Handle, InputValue, LoopSpec, Node, NodeId, NodeKind,
    OutputField, ReportTo, Requirement, Workflow, WorkflowInput, DECIDE_AI, TRIGGERS_MAX,
    WORKFLOWS_MAX,
};
use super::models::{workflow_issues, ModelCatalog};
use super::ordered::OrderedMap;
use super::report::field_in_report;
use super::schedule::{schedule_triggers, scheduled};
use super::template::js_trim;
use super::validate::WorkflowIssue;
use super::yaml::{anchor_node, input_value_problems};

/// What the board says when a flagged workflow is asked to be enabled by a save.
pub const REVIEW_FIRST: &str = "Review this workflow before enabling it.";
/// What the board says when the stored copy is not the one it loaded.
pub const CHANGED_ELSEWHERE: &str = "This workflow changed elsewhere";

/// A loop made by dragging a connection back: three rounds, the AI decides.
pub fn default_loop() -> LoopSpec {
    LoopSpec {
        max_rounds: 3.0,
        decide: DECIDE_AI.into(),
    }
}

/// The parts of a workflow the board keeps next to its nodes and edges.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub name: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub inputs: OrderedMap<WorkflowInput>,
    pub input_values: OrderedMap<InputValue>,
    pub requires: Vec<Requirement>,
    pub decision_model: Option<DecisionModel>,
    pub report_to: Option<ReportTo>,
    pub summary_template: Option<String>,
}

impl Meta {
    pub fn of(w: &Workflow) -> Self {
        Self {
            name: w.name.clone(),
            description: w.description.clone(),
            enabled: w.enabled,
            inputs: w.inputs.clone(),
            input_values: w.input_values.clone(),
            requires: w.requires.clone(),
            decision_model: w.decision_model.clone(),
            report_to: w.report_to.clone(),
            summary_template: w.summary_template.clone(),
        }
    }
}

/// The workflow the board holds now: `base` with the drawn nodes and edges and the meta on it.
pub fn current(base: &Workflow, flow: &Flow, meta: &Meta) -> Workflow {
    let mut w = from_flow(base, &flow.nodes, &flow.edges);
    w.name = meta.name.clone();
    w.description = meta.description.clone();
    w.enabled = meta.enabled;
    w.inputs = meta.inputs.clone();
    w.input_values = meta.input_values.clone();
    w.requires = meta.requires.clone();
    w.decision_model = meta.decision_model.clone();
    w.report_to = meta.report_to.clone();
    w.summary_template = meta.summary_template.clone();
    w
}

/// Everything wrong with a workflow that would keep it from running. A step with no project
/// says so in the words of the field.
pub fn problems_of(w: &Workflow, catalog: &ModelCatalog) -> Vec<WorkflowIssue> {
    let unset: Vec<&str> = w
        .nodes
        .iter()
        .filter(|n| match &n.kind {
            NodeKind::Session { project, .. } | NodeKind::Command { project, .. } => {
                project.is_empty()
            }
            _ => false,
        })
        .map(|n| n.id.as_str())
        .collect();
    let mut issues: Vec<WorkflowIssue> = workflow_issues(w, catalog)
        .into_iter()
        .map(|issue| {
            let missing = issue
                .node_id
                .as_deref()
                .is_some_and(|id| unset.contains(&id))
                && issue.message.starts_with("project is not set");
            if missing {
                WorkflowIssue {
                    message: "Choose a project".into(),
                    ..issue
                }
            } else {
                issue
            }
        })
        .collect();
    issues.extend(
        input_value_problems(&w.inputs, &w.input_values)
            .into_iter()
            .map(|message| WorkflowIssue {
                node_id: None,
                message,
            }),
    );
    issues
}

// ---- saving ------------------------------------------------------------------------------

/// Why a save did not go through.
#[derive(Clone, Debug, PartialEq)]
pub enum SaveRefusal {
    /// The stored copy is not the one the board last saw; here it is.
    Changed(Box<Workflow>),
    /// Said to the user as it is.
    Message(String),
}

/// The stored copy of a workflow with the board's definition on it, the fields the app manages
/// kept. A schedule that is unchanged keeps its place; a new, edited or resumed one starts afresh.
pub fn merged(stored: Option<&Workflow>, next: &Workflow, now: u64) -> Workflow {
    let mut base = match stored {
        Some(stored) => stored.clone(),
        None => {
            let mut fresh = Workflow::new(next.id.clone(), next.name.clone());
            fresh.created_at = next.created_at;
            fresh
        }
    };
    base.next_run_at = None;
    base.id = next.id.clone();
    base.name = next.name.clone();
    base.description = next.description.clone().filter(|d| !js_trim(d).is_empty());
    base.inputs = next.inputs.clone();
    base.input_values = next
        .input_values
        .iter()
        .filter(|(key, _)| next.inputs.contains_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    base.requires = next.requires.clone();
    base.decision_model = next.decision_model.clone();
    base.report_to = next.report_to.clone();
    base.summary_template = next.summary_template.clone();
    base.workspace = next.workspace.clone();
    base.concurrency = next.concurrency;
    base.nodes = next
        .nodes
        .iter()
        .map(|node| anchor_node(node.clone(), now))
        .collect();
    base.edges = next.edges.clone();
    base.enabled = next.enabled;
    base.next_run_at_by_node.clear();
    if schedule_triggers(&base).is_empty() || !base.enabled {
        return base;
    }
    let schedules = |w: &Workflow| -> Vec<(String, crate::mono::habits::Schedule)> {
        schedule_triggers(w)
            .into_iter()
            .map(|(node, schedule)| (node.id.clone(), schedule.clone()))
            .collect()
    };
    let same = stored
        .is_some_and(|s| s.enabled && s.next_run_at.is_some() && schedules(s) == schedules(&base));
    match stored.filter(|_| same) {
        Some(stored) => {
            base.next_run_at = stored.next_run_at;
            base.next_run_at_by_node = stored.next_run_at_by_node.clone();
        }
        None => {
            if let Ok(timed) = scheduled(&base, now) {
                base.next_run_at = timed.next_run_at;
                base.next_run_at_by_node = timed.next_run_at_by_node;
            }
        }
    }
    base
}

/// `list` with the draft saved into it: the new list and the workflow as stored. `seen` is the
/// `definition_key` of the stored copy the board last saw; `overwrite` goes past a stored copy
/// that differs.
pub fn save_into(
    mut list: Vec<Workflow>,
    next: &Workflow,
    overwrite: bool,
    seen: &str,
    now: u64,
) -> Result<(Vec<Workflow>, Workflow), SaveRefusal> {
    let at = list.iter().position(|w| w.id == next.id);
    let stored = at.map(|i| &list[i]);
    // Not over a definition this board has not seen; Overwrite is the explicit way past.
    if let Some(stored) = stored {
        if !overwrite && definition_key(stored) != seen {
            return Err(SaveRefusal::Changed(Box::new(stored.clone())));
        }
    } else if list.len() >= WORKFLOWS_MAX {
        return Err(SaveRefusal::Message(format!(
            "At most {WORKFLOWS_MAX} workflows."
        )));
    }
    // The stored copy decides, not what the board loaded: it may have been flagged since.
    if stored.is_some_and(|s| s.needs_review) && next.enabled {
        return Err(SaveRefusal::Message(REVIEW_FIRST.into()));
    }
    let result = merged(stored, next, now);
    match at {
        Some(i) => list[i] = result.clone(),
        None => list.push(result.clone()),
    }
    Ok((list, result))
}

// ---- the picker -----------------------------------------------------------------------------

pub struct PickerGroup {
    pub title: &'static str,
    pub items: &'static [(NodeType, &'static str)],
}

/// The menu behind "+ Node": every kind of node, grouped by its part in the workflow.
pub fn picker_groups() -> &'static [PickerGroup] {
    &[
        PickerGroup {
            title: "Trigger",
            items: &[
                (NodeType::Schedule, "Runs on a schedule"),
                (NodeType::Manual, "Runs when you start it"),
            ],
        },
        PickerGroup {
            title: "Action",
            items: &[
                (NodeType::Session, "A session works on a project"),
                (NodeType::Command, "A shell command"),
            ],
        },
        PickerGroup {
            title: "Flow",
            items: &[
                (NodeType::If, "Go one way or the other"),
                (NodeType::Switch, "Pick one of several ways"),
                (NodeType::ForEach, "Repeat for each item of a list"),
            ],
        },
    ]
}

/// Why a type cannot be added now, if it cannot.
pub fn picker_unavailable(ty: NodeType, trigger_count: usize) -> Option<&'static str> {
    match ty {
        NodeType::Schedule | NodeType::Manual if trigger_count >= TRIGGERS_MAX => {
            Some("A workflow has at most 5 triggers")
        }
        _ => None,
    }
}

/// `NODE_LABEL` of the board.
pub fn type_label(ty: NodeType) -> &'static str {
    match ty {
        NodeType::Schedule => "Schedule",
        NodeType::Manual => "Manual",
        NodeType::Session => "Session",
        NodeType::Command => "Command",
        NodeType::If => "If",
        NodeType::Switch => "Switch",
        NodeType::ForEach => "For each",
    }
}

// ---- editing the flow -------------------------------------------------------------------------

fn handle_of(name: Option<&str>) -> Option<Handle> {
    name.filter(|n| !n.is_empty()).map(Handle::named)
}

/// Whether a connection from `source` (leaving by the If handle `handle`, if any) to `target`
/// may be made, as a plain edge or as a loop.
pub fn can_connect(w: &Workflow, source: &str, handle: Option<&str>, target: &str) -> bool {
    connection_kind(w, source, target, handle_of(handle)) != ConnectionKind::Invalid
}

/// Whether a loop from `from` back to `to` may be added now (both still there, `to` an ancestor,
/// `from` free to start one).
pub fn loop_allowed(w: &Workflow, from: &str, to: &str) -> bool {
    connection_kind(w, from, to, None) == ConnectionKind::Loop
}

/// The ids of the top-level trigger nodes of a flow.
pub fn trigger_ids(nodes: &[FlowNode]) -> Vec<&str> {
    nodes
        .iter()
        .filter(|n| n.parent_id.is_none() && n.data.node.is_trigger())
        .map(|n| n.id.as_str())
        .collect()
}

/// Whether every trigger has an edge to `target`. The canvas draws one edge from the trigger
/// frame for such a target; an edge to a target only some triggers lead to leaves its own trigger.
pub fn shared_by_all_triggers(nodes: &[FlowNode], edges: &[FlowEdge], target: &str) -> bool {
    trigger_ids(nodes)
        .iter()
        .all(|t| edges.iter().any(|e| e.source == *t && e.target == target))
}

/// The flow with the node (and edge) given selected and nothing else. The canvas draws one edge
/// from the trigger frame for all the edges every trigger has to the same step, so selecting such
/// an edge selects every trigger edge to that step: deleting it removes them all. An edge to a
/// step only some triggers lead to is its own edge and is selected alone.
pub fn select(flow: &Flow, node: Option<&str>, edge: Option<&str>) -> Flow {
    let mut out = flow.clone();
    for n in &mut out.nodes {
        n.selected = node == Some(n.id.as_str());
    }
    let triggers = trigger_ids(&flow.nodes);
    let framed_target = edge
        .and_then(|id| flow.edges.iter().find(|e| e.id == id))
        .filter(|e| triggers.contains(&e.source.as_str()))
        .filter(|e| shared_by_all_triggers(&flow.nodes, &flow.edges, &e.target))
        .map(|e| e.target.clone());
    for e in &mut out.edges {
        e.selected = edge == Some(e.id.as_str())
            || framed_target
                .as_deref()
                .is_some_and(|target| e.target == target && triggers.contains(&e.source.as_str()));
    }
    out
}

fn is_trigger_type(ty: NodeType) -> bool {
    match ty {
        NodeType::Schedule | NodeType::Manual => true,
        NodeType::Session
        | NodeType::Command
        | NodeType::If
        | NodeType::Switch
        | NodeType::ForEach => false,
    }
}

fn sort_parents_first(nodes: Vec<FlowNode>) -> Vec<FlowNode> {
    let (groups, rest): (Vec<_>, Vec<_>) =
        nodes.into_iter().partition(|n| n.kind == FlowKind::Group);
    groups.into_iter().chain(rest).collect()
}

/// `w` (the workflow the flow draws) with a new node of type `ty` added to the flow: where
/// the board puts it (above the triggers, in the selected For each, or under the lowest node of
/// its scope) and wired after the node that led nowhere. A new trigger is wired to the same
/// first step as the triggers already there. Returns the new
/// flow, the new node selected, and its id.
pub fn add_node(flow: &Flow, w: &Workflow, ty: NodeType) -> (Flow, NodeId) {
    let node = new_node(ty, w);
    let selected = flow.nodes.iter().find(|n| n.selected);
    let parent: Option<NodeId> = if groupable(&node) {
        match selected {
            Some(s) if s.kind == FlowKind::Group => Some(s.id.clone()),
            Some(s) => s.parent_id.clone(),
            None => None,
        }
    } else {
        None
    };
    let to_node = |position: Point, parent_id: Option<NodeId>| -> FlowNode {
        let group = is_group(&node);
        FlowNode {
            id: node.id.clone(),
            kind: if group {
                FlowKind::Group
            } else {
                FlowKind::Board
            },
            position,
            width: if group { EMPTY_GROUP_W } else { NODE_W },
            height: if group {
                EMPTY_GROUP_H
            } else {
                node_height(&node)
            },
            parent_id,
            selected: true,
            data: BoardNodeData::new(node.clone()),
        }
    };
    let mut deselected = flow.nodes.clone();
    for n in &mut deselected {
        n.selected = false;
    }
    let mut edges = flow.edges.clone();
    let nodes: Vec<FlowNode>;
    if is_trigger_type(ty) {
        // The triggers sit in one column; the new one goes at its end, and the layout
        // (`stack_triggers`, below) puts it in its slot.
        let count = deselected
            .iter()
            .filter(|n| n.parent_id.is_none() && n.data.node.is_trigger())
            .count();
        let at = match trigger_frame(&deselected) {
            Some((frame, _)) => trigger_slot(frame, count),
            None => {
                // No trigger left: the frame goes before everything else.
                let tops = || deselected.iter().filter(|n| n.parent_id.is_none());
                let left = tops().map(|n| n.position.x).fold(f32::MAX, f32::min);
                let top = tops().map(|n| n.position.y).fold(f32::MAX, f32::min);
                let (left, top) = if left == f32::MAX {
                    (0.0, 0.0)
                } else {
                    (left, top)
                };
                trigger_slot(
                    Point {
                        x: left - GAP_X - trigger_frame_size(1).width,
                        y: top,
                    },
                    0,
                )
            }
        };
        let mut all = deselected;
        all.push(to_node(at, None));
        nodes = all;
        // Any trigger can start a run, so each leads to the same first step.
        let existing = w
            .nodes
            .iter()
            .find(|n| n.is_trigger() && n.parent.is_none());
        if let Some(existing) = existing {
            let from_existing: Vec<FlowEdge> = flow
                .edges
                .iter()
                .filter(|e| e.loop_spec.is_none() && e.source == existing.id)
                .cloned()
                .collect();
            edges.extend(
                from_existing
                    .iter()
                    .map(|e| make_edge(&node.id, &e.target, None, None)),
            );
        }
    } else {
        let place = auto_place(w, parent.as_deref());
        let mut all = deselected;
        all.push(to_node(place, parent.clone()));
        nodes = all;
        // Wired after the node that sits furthest right in its scope, when that one leads
        // nowhere yet.
        let x_of = |n: &Node| n.ui.map_or(0.0, |u| u.x);
        let mut lowest: Option<&Node> = None;
        for n in w.nodes.iter().filter(|n| n.parent == parent) {
            if lowest.is_none_or(|low| x_of(n) >= x_of(low)) {
                lowest = Some(n);
            }
        }
        if let Some(lowest) = lowest {
            let free = !matches!(lowest.kind, NodeKind::If { .. } | NodeKind::Switch { .. })
                && !w
                    .edges
                    .iter()
                    .any(|e| e.loop_spec.is_none() && e.from == lowest.id);
            if free {
                let mut wider = w.clone();
                let mut added = node.clone();
                added.parent = parent.clone();
                wider.nodes.push(added);
                if connection_kind(&wider, &lowest.id, &node.id, None) == ConnectionKind::Edge {
                    // Every trigger leads to the same first step, so all the loose ones go
                    // to a first step together.
                    let sources: Vec<&Node> = if lowest.is_trigger() {
                        w.nodes
                            .iter()
                            .filter(|n| n.is_trigger() && n.parent == parent)
                            .filter(|n| {
                                !w.edges
                                    .iter()
                                    .any(|e| e.loop_spec.is_none() && e.from == n.id)
                            })
                            .collect()
                    } else {
                        vec![lowest]
                    };
                    for source in sources {
                        edges.push(make_edge(&source.id, &node.id, None, None));
                    }
                }
            }
        }
    }
    let placed = sort_parents_first(nodes);
    let fitted = stack_triggers(push_right(&placed, &fit_groups(&placed)), None);
    (
        Flow {
            nodes: fitted,
            edges,
        },
        node.id,
    )
}

/// What dragging a connection from `source` to `target` makes.
pub enum Connected {
    Invalid,
    /// A way back to an ancestor: the board asks for the loop's rounds and decider first.
    Loop,
    Edge(Flow),
}

/// Connects `source` to `target`; `handle` is the If handle ("true" or "false") it left from.
pub fn connect(
    flow: &Flow,
    w: &Workflow,
    source: &str,
    target: &str,
    handle: Option<&str>,
) -> Connected {
    let handle = handle_of(handle);
    match connection_kind(w, source, target, handle.clone()) {
        ConnectionKind::Invalid => Connected::Invalid,
        ConnectionKind::Loop => Connected::Loop,
        ConnectionKind::Edge => {
            let mut out = flow.clone();
            // An If node's true or false, and each handle of a Switch, leads to one place.
            if let Some(h) = &handle {
                out.edges.retain(|e| {
                    !(e.loop_spec.is_none()
                        && e.source == source
                        && e.source_handle.as_deref() == Some(h.as_str()))
                });
            }
            out.edges.push(make_edge(source, target, handle, None));
            Connected::Edge(out)
        }
    }
}

/// The flow with the loop `from` -> `to` added.
pub fn add_loop(flow: &Flow, from: &str, to: &str, spec: LoopSpec) -> Flow {
    let mut out = flow.clone();
    out.edges.push(make_edge(from, to, None, Some(spec)));
    out
}

/// Changes the loop that leaves node `id`, or takes it away with None.
pub fn set_loop(flow: &Flow, id: &str, spec: Option<LoopSpec>) -> Flow {
    let Some(at) = flow
        .edges
        .iter()
        .position(|e| e.loop_spec.is_some() && e.source == id)
    else {
        return flow.clone();
    };
    let mut out = flow.clone();
    match spec {
        Some(spec) => {
            let old = &flow.edges[at];
            out.edges[at] = make_edge(&old.source, &old.target, None, Some(spec));
        }
        None => {
            out.edges.remove(at);
        }
    }
    out
}

/// The node `next` in place of the one with its id.
pub fn update_node(flow: &Flow, next: &Node) -> Flow {
    let mut out = flow.clone();
    for n in &mut out.nodes {
        if n.id == next.id {
            n.data.node = next.clone();
            n.height = node_height(next);
        }
    }
    out
}

/// The flow laid out afresh, left to right: every saved position (and the board's own) is
/// dropped and the layout runs again over `w`, the workflow the flow draws. Selection stays.
pub fn tidy(flow: &Flow, w: &Workflow) -> Flow {
    let mut fresh = from_flow(w, &flow.nodes, &flow.edges);
    for n in &mut fresh.nodes {
        n.ui = None;
    }
    let mut out = to_flow(&fresh);
    for n in &mut out.nodes {
        n.selected = flow.nodes.iter().any(|o| o.id == n.id && o.selected);
        n.data.issues = flow
            .nodes
            .iter()
            .find(|o| o.id == n.id)
            .map(|o| o.data.issues.clone())
            .unwrap_or_default();
    }
    for e in &mut out.edges {
        e.selected = flow.edges.iter().any(|o| o.id == e.id && o.selected);
    }
    out
}

/// After the node `id` was dragged: it joins the For each it was dropped on or leaves the one it
/// was in, and a node that changed frame is no longer wired across it.
pub fn drag_stop(flow: &Flow, id: &str) -> Flow {
    let nodes = settle_drop(&flow.nodes, id);
    let parent = |list: &[FlowNode]| {
        list.iter()
            .find(|n| n.id == id)
            .and_then(|n| n.parent_id.clone())
    };
    let moved = parent(&flow.nodes) != parent(&nodes);
    let edges = if moved {
        drop_crossing_edges(&nodes, &flow.edges, &[id.to_string()])
    } else {
        flow.edges.clone()
    };
    Flow { nodes, edges }
}

/// The flow without the nodes (and the wires) in `ids`.
pub fn remove_node(flow: &Flow, id: &str) -> Flow {
    drop_nodes(&flow.nodes, &flow.edges, &[id.to_string()])
}

/// What the Delete key removes: the selected nodes (a For each takes only its frame) and edges.
pub fn delete_selected(flow: &Flow) -> Flow {
    let picked: Vec<&str> = flow
        .nodes
        .iter()
        .filter(|n| n.selected)
        .map(|n| n.id.as_str())
        .collect();
    // The canvas lists the children of a deleted For each too.
    let doomed: Vec<FlowNode> = flow
        .nodes
        .iter()
        .filter(|n| n.selected || n.parent_id.as_deref().is_some_and(|p| picked.contains(&p)))
        .cloned()
        .collect();
    let touching: Vec<_> = flow
        .edges
        .iter()
        .filter(|e| e.selected || doomed.iter().any(|n| n.id == e.source || n.id == e.target))
        .cloned()
        .collect();
    if doomed.is_empty() && touching.is_empty() {
        return flow.clone();
    }
    let gone = deletion(&doomed, &touching);
    let ids: Vec<NodeId> = gone.nodes.iter().map(|n| n.id.clone()).collect();
    let mut out = drop_nodes(&flow.nodes, &flow.edges, &ids);
    out.edges
        .retain(|e| !gone.edges.iter().any(|g| g.id == e.id));
    out
}

// ---- the inspector's fields --------------------------------------------------------------------

/// The input a `{{inputs.x}}` reference names.
pub fn project_ref_name(project: &str) -> Option<&str> {
    let name = project.strip_prefix("{{inputs.")?.strip_suffix("}}")?;
    usable_name(name).then_some(name)
}

/// The path a step's project reference stands for: the value of the input it names.
pub fn project_path(w: &Workflow, node: &Node) -> Option<String> {
    let (NodeKind::Session { project, .. } | NodeKind::Command { project, .. }) = &node.kind else {
        return None;
    };
    match w.input_values.get(project_ref_name(project)?) {
        Some(InputValue::String(path)) => Some(path.clone()),
        _ => None,
    }
}

/// A step bound to a project: the node, and the inputs and values of the workflow with it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectChoice {
    pub node: Node,
    pub inputs: OrderedMap<WorkflowInput>,
    pub input_values: OrderedMap<InputValue>,
}

fn with_project(node: &Node, reference: &str) -> Node {
    let mut out = node.clone();
    if let NodeKind::Session { project, .. } | NodeKind::Command { project, .. } = &mut out.kind {
        *project = reference.to_string();
    }
    out
}

/// Choosing the project `path` (called `label`) for a step: the step names an input that holds
/// it, and an input the step left goes with it when nothing else mentions it.
pub fn project_choice(w: &Workflow, node: &Node, path: &str, label: &str) -> ProjectChoice {
    let (NodeKind::Session { project, .. } | NodeKind::Command { project, .. }) = &node.kind else {
        return ProjectChoice {
            node: node.clone(),
            inputs: w.inputs.clone(),
            input_values: w.input_values.clone(),
        };
    };
    let name = project_ref_name(project);
    let made = project_input_for(w, path, label, name);
    let mut inputs = made.inputs;
    let mut input_values = made.input_values;
    if let Some(name) = name {
        let is_project = matches!(
            inputs.get(name),
            Some(i) if i.ty == super::model::InputType::Project
        );
        if made.reference != *project && is_project {
            // Loop conditions mention inputs too, so the edges are searched along with the nodes.
            let nodes: Vec<Node> = w
                .nodes
                .iter()
                .map(|n| {
                    if n.id == node.id {
                        with_project(n, "")
                    } else {
                        n.clone()
                    }
                })
                .collect();
            let rest = serde_json::json!({"nodes": nodes, "edges": w.edges}).to_string();
            if !rest.contains(&format!("inputs.{name}")) {
                inputs.remove(name);
                input_values.remove(name);
            }
        }
    }
    ProjectChoice {
        node: with_project(node, &made.reference),
        inputs,
        input_values,
    }
}

/// A plain identifier: letters, digits and underscores, not starting with a digit.
pub fn usable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name != "__proto__"
}

/// What is wrong with the name typed for a new input, if anything.
pub fn input_name_problem(name: &str, taken: &dyn Fn(&str) -> bool) -> Option<String> {
    if name.is_empty() {
        None
    } else if !usable_name(name) {
        Some("Use letters, digits and underscores".into())
    } else if taken(name) {
        Some("Already declared".into())
    } else {
        None
    }
}

/// The name a new output field starts with.
pub fn next_field_name(fields: &OrderedMap<OutputField>) -> String {
    let mut name = "field".to_string();
    let mut n = 2;
    while fields.contains_key(&name) {
        name = format!("field_{n}");
        n += 1;
    }
    name
}

/// The fields with `from` called `to`, in the same place.
pub fn rename_field(
    fields: &OrderedMap<OutputField>,
    from: &str,
    to: &str,
) -> OrderedMap<OutputField> {
    fields
        .iter()
        .map(|(key, field)| {
            (
                if key == from {
                    to.to_string()
                } else {
                    key.clone()
                },
                field.clone(),
            )
        })
        .collect()
}

/// The fields with "In report" of `key` set to the opposite of what it effectively is.
pub fn toggle_in_report(fields: &OrderedMap<OutputField>, key: &str) -> OrderedMap<OutputField> {
    fields
        .iter()
        .map(|(k, field)| {
            let mut field = field.clone();
            if k == key {
                field.in_report = Some(!field_in_report(&field));
            }
            (k.clone(), field)
        })
        .collect()
}

pub fn checkout_value(checkout: &Checkout) -> String {
    match checkout {
        Checkout::Run => "run".into(),
        Checkout::Root => "root".into(),
        Checkout::NewWorktree => "newWorktree".into(),
        Checkout::WorktreeOf(id) => format!("worktree:{id}"),
        Checkout::Existing(path) => format!("existing:{path}"),
    }
}

pub fn parse_checkout(value: &str) -> Checkout {
    if let Some(id) = value.strip_prefix("worktree:") {
        return Checkout::WorktreeOf(id.to_string());
    }
    if let Some(path) = value.strip_prefix("existing:") {
        return Checkout::Existing(path.to_string());
    }
    match value {
        "run" => Checkout::Run,
        "newWorktree" => Checkout::NewWorktree,
        _ => Checkout::Root,
    }
}

/// No cwd means the run worktree (the project folder when the workspace makes none).
pub fn cwd_value(cwd: Option<&Cwd>) -> String {
    match cwd {
        None => "run".into(),
        Some(Cwd::Root) => "root".into(),
        Some(Cwd::WorktreeOf(id)) => format!("worktree:{id}"),
    }
}

pub fn parse_cwd(value: &str) -> Option<Cwd> {
    match value.strip_prefix("worktree:") {
        Some(id) => Some(Cwd::WorktreeOf(id.to_string())),
        None if value == "root" => Some(Cwd::Root),
        None => None,
    }
}

/// The session nodes other than `except`: what a worktree or a nest can be of.
pub fn sessions_except<'a>(w: &'a Workflow, except: &str) -> Vec<&'a Node> {
    w.nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::Session { .. }) && n.id != except)
        .collect()
}

/// Whether the node is in the body of a loop (the engine runs it again each round).
pub fn in_loop(w: &Workflow, id: &str) -> bool {
    looped_node_ids(w).contains(id)
}

/// An If node decided by the AI or by a condition (which starts empty).
pub fn if_decided_by(node: &Node, ai: bool) -> Node {
    let mut out = node.clone();
    if let NodeKind::If {
        condition, decide, ..
    } = &mut out.kind
    {
        if ai {
            *condition = None;
            *decide = Some(Decide::Ai);
        } else {
            *decide = None;
            *condition = Some(String::new());
        }
    }
    out
}

/// A Switch decided by the AI or by a value (which starts empty).
pub fn switch_decided_by(node: &Node, ai: bool) -> Node {
    let mut out = node.clone();
    if let NodeKind::Switch { value, decide, .. } = &mut out.kind {
        if ai {
            *value = None;
            *decide = Some(Decide::Ai);
        } else {
            *decide = None;
            *value = Some(String::new());
        }
    }
    out
}

/// The flow with the Switch `id` changed by `edit` on its cases, and every out edge of it
/// passed through `edges` (which keeps, renames or drops it).
fn edit_switch(
    flow: &Flow,
    id: &str,
    edit: impl FnOnce(&mut Vec<String>),
    edges: impl Fn(&FlowEdge) -> Option<Option<Handle>>,
) -> Flow {
    let mut out = flow.clone();
    let Some(node) = out.nodes.iter_mut().find(|n| n.id == id) else {
        return out;
    };
    let NodeKind::Switch { cases, .. } = &mut node.data.node.kind else {
        return out;
    };
    edit(cases);
    node.height = node_height(&node.data.node);
    out.edges = flow
        .edges
        .iter()
        .filter_map(|e| {
            if e.loop_spec.is_some() || e.source != id {
                return Some(e.clone());
            }
            let handle = edges(e)?;
            let mut next = make_edge(&e.source, &e.target, handle, None);
            next.selected = e.selected;
            Some(next)
        })
        .collect();
    out
}

/// The Switch `id` with `case` added as its last case. A case already there is not added twice.
pub fn add_switch_case(flow: &Flow, id: &str, case: &str) -> Flow {
    edit_switch(
        flow,
        id,
        |cases| {
            if !cases.iter().any(|c| c == case) {
                cases.push(case.to_string());
            }
        },
        |e| Some(e.source_handle.as_deref().map(Handle::named)),
    )
}

/// Whether a line leaving the node `id` already uses `name` as its handle.
pub fn out_edge_uses_handle(flow: &Flow, id: &str, name: &str) -> bool {
    flow.edges.iter().any(|e| {
        e.loop_spec.is_none() && e.source == id && e.source_handle.as_deref() == Some(name)
    })
}

/// The Switch `id` with the case `from` called `to`, in the same place; the edges that left by
/// `from` leave by `to`. A `to` that a line of the node already leaves by (and `from` does not)
/// would capture that line, so nothing changes then.
pub fn rename_switch_case(flow: &Flow, id: &str, from: &str, to: &str) -> Flow {
    if from != to && out_edge_uses_handle(flow, id, to) {
        return flow.clone();
    }
    edit_switch(
        flow,
        id,
        |cases| {
            for c in cases.iter_mut().filter(|c| *c == from) {
                *c = to.to_string();
            }
        },
        |e| {
            Some(e.source_handle.as_deref().map(|h| {
                if h == from {
                    Handle::named(to)
                } else {
                    Handle::named(h)
                }
            }))
        },
    )
}

/// The Switch `id` without the case `case`; the edges that left by it are removed with it.
pub fn remove_switch_case(flow: &Flow, id: &str, case: &str) -> Flow {
    edit_switch(
        flow,
        id,
        |cases| cases.retain(|c| c != case),
        |e| match e.source_handle.as_deref() {
            Some(h) if h == case => None,
            other => Some(other.map(Handle::named)),
        },
    )
}

/// A loop decided by the AI or by a condition (which starts empty).
pub fn loop_decided_by(spec: &LoopSpec, ai: bool) -> LoopSpec {
    let decide = if ai {
        DECIDE_AI.to_string()
    } else if super::parse::is_ai_decide(&spec.decide) {
        String::new()
    } else {
        spec.decide.clone()
    };
    LoopSpec {
        decide,
        ..spec.clone()
    }
}
