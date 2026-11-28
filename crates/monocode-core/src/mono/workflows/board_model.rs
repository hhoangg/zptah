//! The board's pure side: a workflow as flow nodes and edges and back, the rules for what may be
//! connected, and where things sit. Port of `boardModel.ts`; no UI types, so the GPUI board and
//! run graph draw from these structs. A For each is a group node; its children carry `parent_id`
//! and a position relative to it, which is what `node.ui` stores for them too. Coordinates are
//! f32 pixels with the same numbers as the React board.
use super::model::{
    new_workflow_id, stable_json, Checkout, Cwd, Decide, Edge, Handle, InputType, InputValue,
    LoopSpec, Node, NodeId, NodeKind, Ui, Workflow, WorkflowInput, WorkflowUi, SWITCH_DEFAULT,
};
use super::ordered::OrderedMap;
use super::run_graph_model::{RunEdgeState, RunNodeState};
use super::template::js_trim;
use super::validate::WorkflowIssue;
use crate::mono::habits::Schedule;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// A node card is this wide and about this tall; layout counts on it.
pub const NODE_W: f32 = 232.0;
pub const NODE_H: f32 = 48.0;
/// Space between a node and the next one in its column, and between one column and the next
/// (which has to hold the arrow of the edges between them).
pub const GAP_Y: f32 = 24.0;
pub const GAP_X: f32 = 72.0;
/// Space between two triggers in their column.
pub const TRIGGER_GAP: f32 = 12.0;
/// Room around the children of a group, and above them for its title.
pub const GROUP_PAD: f32 = 16.0;
pub const GROUP_HEADER: f32 = 32.0;
/// Space left after the last child, so another can be dropped in.
pub const GROUP_SLACK: f32 = 44.0;
/// A loop edge runs under the children of a group, so the frame has room below them.
pub const GROUP_LOOP: f32 = 36.0;

/// How far apart an If card puts its two outs (true at 28% and false at 72% of its height), and
/// the share of its height a card with three or more outs spreads them over (12% to 88%).
const IF_OUT_SPACING: f32 = NODE_H * (0.72 - 0.28);
const MANY_OUTS_SPAN: f32 = 0.88 - 0.12;

/// The height of the card of `node`: `NODE_H`, and for a Switch as much as its outs need to sit
/// at least as far apart as an If's two (more cases make a taller card).
pub fn node_height(node: &Node) -> f32 {
    match &node.kind {
        NodeKind::Switch { cases, .. } if cases.len() >= 2 => {
            let gaps = cases.len() as f32;
            NODE_H.max((gaps * IF_OUT_SPACING / MANY_OUTS_SPAN).ceil())
        }
        _ => NODE_H,
    }
}

/// The size of a For each with nothing in it yet.
pub const EMPTY_GROUP_W: f32 = GROUP_PAD + NODE_W + GROUP_PAD + GROUP_SLACK;
pub const EMPTY_GROUP_H: f32 = GROUP_HEADER + NODE_H + GROUP_PAD + GROUP_LOOP;

/// Handle ids: the two outs of an If node ("true", "false"), the outs of a Switch (each case,
/// and `SWITCH_DEFAULT`), and the side handles a loop edge uses.
pub const LOOP_OUT: &str = "loop-out";
pub const LOOP_IN: &str = "loop-in";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

/// Which card a flow node is drawn as: a node card, or the frame of a For each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowKind {
    Board,
    Group,
}

/// What a flow node carries besides its place. Everything but `node` is set when drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardNodeData {
    pub node: Node,
    /// What `validate_workflow` found for this node.
    pub issues: Vec<String>,
    /// Whether a loop edge enters or leaves the node.
    pub loop_in: bool,
    pub loop_out: bool,
    /// For a For each: nothing is in it yet.
    pub empty: bool,
    /// Run graph only: what the run did with the node.
    pub run: Option<RunNodeState>,
    /// Run graph only: a narrow card (no icon tile) for lanes side by side.
    pub compact: bool,
    /// Run graph, For each: how many items its selected round has.
    pub run_items: Option<usize>,
}

impl BoardNodeData {
    pub fn new(node: Node) -> Self {
        Self {
            node,
            issues: Vec::new(),
            loop_in: false,
            loop_out: false,
            empty: false,
            run: None,
            compact: false,
            run_items: None,
        }
    }
}

/// A node on the canvas. `position` is relative to `parent_id` when there is one; a child is
/// always confined to its parent (the xyflow `extent: "parent"`). `width` and `height` are the
/// nominal card size (`NODE_W` x `NODE_H`) or, for a group, the frame size from the layout.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowNode {
    pub id: NodeId,
    pub kind: FlowKind,
    pub position: Point,
    pub width: f32,
    pub height: f32,
    pub parent_id: Option<NodeId>,
    pub selected: bool,
    pub data: BoardNodeData,
}

impl FlowNode {
    /// A node card at `position`, not in a group.
    pub fn new_board(node: Node, position: Point) -> Self {
        Self {
            id: node.id.clone(),
            kind: FlowKind::Board,
            position,
            width: NODE_W,
            height: node_height(&node),
            parent_id: None,
            selected: false,
            data: BoardNodeData::new(node),
        }
    }

    pub fn size(&self) -> Size {
        Size {
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    /// Smooth-step orthogonal with an arrowhead.
    Smoothstep,
    /// The dashed loop edge from the right of the source back to the left of the target.
    Loop,
}

/// An edge on the canvas. `source_handle` is "true"/"false" on an If out edge, a case or
/// "default" on a Switch out edge, and `LOOP_OUT` on a loop edge (whose `target_handle` is `LOOP_IN`). `run` is set only in the run graph, and
/// `compact` only on the edges of lanes drawn side by side.
#[derive(Clone, Debug, PartialEq)]
pub struct FlowEdge {
    pub id: String,
    pub kind: EdgeKind,
    pub source: NodeId,
    pub target: NodeId,
    pub source_handle: Option<String>,
    pub target_handle: Option<&'static str>,
    pub loop_spec: Option<LoopSpec>,
    pub compact: bool,
    pub run: Option<RunEdgeState>,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flow {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

/// The node types, for the picker and `new_node`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeType {
    Schedule,
    Manual,
    Session,
    Command,
    If,
    Switch,
    ForEach,
}

impl NodeType {
    pub const ALL: [NodeType; 7] = [
        Self::Schedule,
        Self::Manual,
        Self::Session,
        Self::Command,
        Self::If,
        Self::Switch,
        Self::ForEach,
    ];

    /// The `type` tag as written in the file.
    pub fn name(self) -> &'static str {
        match self {
            Self::Schedule => "schedule",
            Self::Manual => "manual",
            Self::Session => "session",
            Self::Command => "command",
            Self::If => "if",
            Self::Switch => "switch",
            Self::ForEach => "forEach",
        }
    }

    pub fn of(kind: &NodeKind) -> Self {
        match kind {
            NodeKind::Schedule { .. } => Self::Schedule,
            NodeKind::Manual => Self::Manual,
            NodeKind::Session { .. } => Self::Session,
            NodeKind::Command { .. } => Self::Command,
            NodeKind::If { .. } => Self::If,
            NodeKind::Switch { .. } => Self::Switch,
            NodeKind::ForEach { .. } => Self::ForEach,
        }
    }

    /// The start of the id a new node of this type gets.
    fn id_base(self) -> &'static str {
        match self {
            Self::Schedule | Self::Manual => "start",
            Self::Session => "session",
            Self::Command => "command",
            Self::If => "check",
            Self::Switch => "route",
            Self::ForEach => "each",
        }
    }
}

/// The out handles a node draws: "true" and "false" for an If; the cases and then "default" for
/// a Switch; none for any other node.
pub fn out_handles(node: &Node) -> Vec<String> {
    match &node.kind {
        NodeKind::If { .. } => vec!["true".into(), "false".into()],
        NodeKind::Switch { cases, .. } => cases
            .iter()
            .cloned()
            .chain(std::iter::once(SWITCH_DEFAULT.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

pub fn is_group(node: &Node) -> bool {
    matches!(node.kind, NodeKind::ForEach { .. })
}

/// Whether a node may sit inside a For each.
pub fn groupable(node: &Node) -> bool {
    !is_group(node) && !node.is_trigger()
}

fn point_of(ui: Ui) -> Point {
    Point {
        x: ui.x as f32,
        y: ui.y as f32,
    }
}

// --- layout ---------------------------------------------------------------------------------

/// The frame of a group around the children at `children` (positions relative to it).
fn group_size(children: &[Point]) -> Size {
    if children.is_empty() {
        return Size {
            width: EMPTY_GROUP_W,
            height: EMPTY_GROUP_H,
        };
    }
    let right = children
        .iter()
        .map(|p| p.x + NODE_W)
        .fold(f32::MIN, f32::max);
    let bottom = children
        .iter()
        .map(|p| p.y + NODE_H)
        .fold(f32::MIN, f32::max);
    Size {
        width: EMPTY_GROUP_W.max(right + GROUP_PAD + GROUP_SLACK),
        height: EMPTY_GROUP_H.max(bottom + GROUP_PAD + GROUP_LOOP),
    }
}

/// The size of the frame the triggers sit in: one column of `count` cards under a title.
pub fn trigger_frame_size(count: usize) -> Size {
    let count = count.max(1) as f32;
    Size {
        width: NODE_W + GROUP_PAD * 2.0,
        height: GROUP_HEADER + count * NODE_H + (count - 1.0) * TRIGGER_GAP + GROUP_PAD,
    }
}

/// Where the `index`th trigger sits in a frame whose top left corner is `frame`.
pub fn trigger_slot(frame: Point, index: usize) -> Point {
    Point {
        x: frame.x + GROUP_PAD,
        y: frame.y + GROUP_HEADER + index as f32 * (NODE_H + TRIGGER_GAP),
    }
}

fn is_top_trigger(n: &FlowNode) -> bool {
    n.parent_id.is_none() && n.data.node.is_trigger()
}

/// The frame drawn behind the triggers: its top left corner and size, from where the triggers
/// are. None when the workflow has no trigger. Pure: the same triggers always give the same frame.
pub fn trigger_frame(nodes: &[FlowNode]) -> Option<(Point, Size)> {
    let triggers: Vec<&FlowNode> = nodes.iter().filter(|n| is_top_trigger(n)).collect();
    if triggers.is_empty() {
        return None;
    }
    let x = triggers
        .iter()
        .map(|n| n.position.x)
        .fold(f32::MAX, f32::min)
        - GROUP_PAD;
    let y = triggers
        .iter()
        .map(|n| n.position.y)
        .fold(f32::MAX, f32::min)
        - GROUP_HEADER;
    Some((Point { x, y }, trigger_frame_size(triggers.len())))
}

/// The triggers put back in one column, in list order, under the frame the triggers other than
/// `moved` (one that was just dragged) make. Their places belong to the layout, so dragging
/// one, adding one or deleting one ends here.
pub fn stack_triggers(nodes: Vec<FlowNode>, moved: Option<&str>) -> Vec<FlowNode> {
    let still: Vec<FlowNode> = nodes
        .iter()
        .filter(|n| Some(n.id.as_str()) != moved)
        .cloned()
        .collect();
    let Some((frame, _)) = trigger_frame(&still).or_else(|| trigger_frame(&nodes)) else {
        return nodes;
    };
    let mut index = 0;
    nodes
        .into_iter()
        .map(|mut n| {
            if is_top_trigger(&n) {
                n.position = trigger_slot(frame, index);
                index += 1;
            }
            n
        })
        .collect()
}

/// The id of the For each a node sits in, if there is one that exists.
fn group_of<'a>(w: &Workflow, node: &'a Node) -> Option<&'a str> {
    let parent = node.parent.as_deref()?;
    if is_group(node) {
        return None;
    }
    w.nodes
        .iter()
        .any(|n| n.id == parent && is_group(n))
        .then_some(parent)
}

/// What `layout_scope` made: where the nodes go, and where the lead frame (the triggers) goes.
struct Placed {
    at: HashMap<NodeId, Point>,
    lead: Point,
}

/// Left to right layers for the nodes of one scope: a node sits one layer after the last node
/// that leads to it (a loop edge is a way back and does not count), the layers are columns, and
/// each column is centred on the tallest one. `lead` is a frame that sits before the first
/// column (the triggers), centred the same way. Sizes are `NODE_W` x `NODE_H` unless `sizes`
/// says otherwise (a group).
fn layout_scope(
    w: &Workflow,
    members: &[&Node],
    origin: Point,
    sizes: &HashMap<NodeId, Size>,
    lead: Option<Size>,
) -> Placed {
    let ids: HashSet<&str> = members.iter().map(|n| n.id.as_str()).collect();
    let edges: Vec<&Edge> = w
        .edges
        .iter()
        .filter(|e| {
            e.loop_spec.is_none() && ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())
        })
        .collect();
    let mut depth: HashMap<&str, usize> = members.iter().map(|n| (n.id.as_str(), 0)).collect();
    // A cycle without a loop edge is a validation problem; the pass count keeps it finite.
    for _ in 0..members.len() {
        let mut moved = false;
        for e in &edges {
            let next = depth.get(e.from.as_str()).copied().unwrap_or(0) + 1;
            if next > depth.get(e.to.as_str()).copied().unwrap_or(0) && next < members.len() {
                depth.insert(e.to.as_str(), next);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    let mut layers: Vec<Vec<&Node>> = Vec::new();
    for n in members {
        let d = depth.get(n.id.as_str()).copied().unwrap_or(0);
        if layers.len() <= d {
            layers.resize_with(d + 1, Vec::new);
        }
        layers[d].push(n);
    }
    layers.retain(|l| !l.is_empty());
    // Within a column, a node goes near the nodes that lead to it, which keeps edges apart.
    let mut row: HashMap<&str, f32> = HashMap::new();
    for layer in layers.iter_mut() {
        let key = |n: &Node| -> f32 {
            let rows: Vec<f32> = edges
                .iter()
                .filter(|e| e.to == n.id)
                .filter_map(|e| row.get(e.from.as_str()).copied())
                .collect();
            if rows.is_empty() {
                f32::MAX
            } else {
                rows.iter().sum::<f32>() / rows.len() as f32
            }
        };
        let keys: HashMap<&str, f32> = layer.iter().map(|n| (n.id.as_str(), key(n))).collect();
        layer.sort_by(|a, b| keys[a.id.as_str()].total_cmp(&keys[b.id.as_str()]));
        for (i, n) in layer.iter().enumerate() {
            row.insert(n.id.as_str(), i as f32);
        }
    }
    let size_of = |n: &Node| {
        sizes.get(&n.id).copied().unwrap_or(Size {
            width: NODE_W,
            height: node_height(n),
        })
    };
    let stack_h = |layer: &Vec<&Node>| -> f32 {
        layer.iter().map(|n| size_of(n).height).sum::<f32>()
            + GAP_Y * layer.len().saturating_sub(1) as f32
    };
    let total = layers
        .iter()
        .map(stack_h)
        .chain(lead.map(|s| s.height))
        .fold(0.0, f32::max);
    let mut out = HashMap::new();
    let mut x = origin.x;
    if let Some(lead) = lead {
        x += lead.width + GAP_X;
    }
    for layer in &layers {
        let mut y = origin.y + (total - stack_h(layer)) / 2.0;
        let mut widest = 0.0_f32;
        for n in layer {
            let s = size_of(n);
            out.insert(n.id.clone(), Point { x, y });
            y += s.height + GAP_Y;
            widest = widest.max(s.width);
        }
        x += widest + GAP_X;
    }
    Placed {
        at: out,
        lead: Point {
            x: origin.x,
            y: origin.y + (total - lead.map_or(0.0, |s| s.height)) / 2.0,
        },
    }
}

struct Layout {
    at: HashMap<NodeId, Point>,
    size: HashMap<NodeId, Size>,
}

/// Where every node sits. The saved position is kept only when the workflow is marked as laid
/// out left to right (`ui.direction`); older top-to-bottom positions are dropped and the whole
/// board is laid out afresh, once, until the next save writes the marker. The triggers always
/// sit in their one column, whatever was saved.
fn layout_of(w: &Workflow) -> Layout {
    let keep = w.is_lr();
    let saved = |n: &Node| n.ui.filter(|_| keep).map(point_of);
    let mut at: HashMap<NodeId, Point> = HashMap::new();
    let mut size: HashMap<NodeId, Size> = HashMap::new();
    let none = HashMap::new();
    for group in w.nodes.iter().filter(|n| is_group(n)) {
        let children: Vec<&Node> = w
            .nodes
            .iter()
            .filter(|n| group_of(w, n) == Some(group.id.as_str()))
            .collect();
        let origin = Point {
            x: GROUP_PAD,
            y: GROUP_HEADER,
        };
        let auto = layout_scope(w, &children, origin, &none, None).at;
        for child in &children {
            let p = saved(child)
                .or_else(|| auto.get(&child.id).copied())
                .unwrap_or(origin);
            at.insert(child.id.clone(), p);
        }
        let points: Vec<Point> = children.iter().map(|c| at[&c.id]).collect();
        size.insert(group.id.clone(), group_size(&points));
    }
    let top: Vec<&Node> = w
        .nodes
        .iter()
        .filter(|n| group_of(w, n).is_none())
        .collect();
    let triggers: Vec<&Node> = top.iter().copied().filter(|n| n.is_trigger()).collect();
    let rest: Vec<&Node> = top.iter().copied().filter(|n| !n.is_trigger()).collect();
    let lead = (!triggers.is_empty()).then(|| trigger_frame_size(triggers.len()));
    let placed = layout_scope(w, &rest, Point::default(), &size, lead);
    for (i, node) in triggers.iter().enumerate() {
        at.insert(node.id.clone(), trigger_slot(placed.lead, i));
    }
    for node in rest {
        let p = saved(node)
            .or_else(|| placed.at.get(&node.id).copied())
            .unwrap_or_default();
        at.insert(node.id.clone(), p);
    }
    Layout { at, size }
}

/// Where a new node goes: after the rightmost one of its scope, level with it. `parent` is the
/// For each it goes into, positions then being relative to it. With only triggers it goes after
/// their frame.
pub fn auto_place(w: &Workflow, parent: Option<&str>) -> Point {
    let Layout { at, size } = layout_of(w);
    let in_scope: Vec<&Node> = w
        .nodes
        .iter()
        .filter(|n| group_of(w, n) == parent && !(parent.is_none() && n.is_trigger()))
        .collect();
    let size_of = |n: &Node| {
        size.get(&n.id).copied().unwrap_or(Size {
            width: NODE_W,
            height: NODE_H,
        })
    };
    let right_of = |n: &Node| at.get(&n.id).map_or(0.0, |p| p.x) + size_of(n).width;
    let mut rightmost: Option<&Node> = None;
    for n in &in_scope {
        if rightmost.is_none_or(|r| right_of(n) >= right_of(r)) {
            rightmost = Some(n);
        }
    }
    if let Some(r) = rightmost {
        let (p, s) = (at[&r.id], size_of(r));
        return Point {
            x: p.x + s.width + GAP_X,
            y: p.y + (s.height - NODE_H) / 2.0,
        };
    }
    if parent.is_some() {
        return Point {
            x: GROUP_PAD,
            y: GROUP_HEADER,
        };
    }
    // Only triggers: after their frame.
    let slots: Vec<Point> = w
        .nodes
        .iter()
        .filter(|n| n.is_trigger() && group_of(w, n).is_none())
        .filter_map(|n| at.get(&n.id).copied())
        .collect();
    if slots.is_empty() {
        return Point::default();
    }
    let top = slots.iter().map(|p| p.y).fold(f32::MAX, f32::min) - GROUP_HEADER;
    let frame = trigger_frame_size(slots.len());
    let left = slots.iter().map(|p| p.x).fold(f32::MAX, f32::min) - GROUP_PAD;
    Point {
        x: left + frame.width + GAP_X,
        y: top + (frame.height - NODE_H) / 2.0,
    }
}

// --- the workflow as a flow --------------------------------------------------------------------

fn edge_id(from: &str, to: &str, handle: Option<&Handle>, looped: bool) -> String {
    let handle = handle
        .map(|h| format!(".{}", h.as_str()))
        .unwrap_or_default();
    format!("{from}{handle}->{to}{}", if looped { "~loop" } else { "" })
}

pub fn make_edge(
    from: &str,
    to: &str,
    handle: Option<Handle>,
    loop_spec: Option<LoopSpec>,
) -> FlowEdge {
    let id = edge_id(from, to, handle.as_ref(), loop_spec.is_some());
    let looped = loop_spec.is_some();
    FlowEdge {
        id,
        kind: if looped {
            EdgeKind::Loop
        } else {
            EdgeKind::Smoothstep
        },
        source: from.to_string(),
        target: to.to_string(),
        source_handle: if looped {
            Some(LOOP_OUT.to_string())
        } else {
            handle.map(|h| h.as_str().to_string())
        },
        target_handle: looped.then_some(LOOP_IN),
        loop_spec,
        compact: false,
        run: None,
        selected: false,
    }
}

/// The canvas wants a group before the nodes inside it.
fn parents_first(nodes: Vec<FlowNode>) -> Vec<FlowNode> {
    let (groups, rest): (Vec<FlowNode>, Vec<FlowNode>) =
        nodes.into_iter().partition(|n| n.kind == FlowKind::Group);
    groups.into_iter().chain(rest).collect()
}

pub fn to_flow(w: &Workflow) -> Flow {
    let Layout { at, size } = layout_of(w);
    let nodes: Vec<FlowNode> = w
        .nodes
        .iter()
        .map(|node| {
            let position = at.get(&node.id).copied().unwrap_or_default();
            let mut flow = FlowNode::new_board(node.clone(), position);
            if is_group(node) {
                let s = size
                    .get(&node.id)
                    .copied()
                    .unwrap_or_else(|| group_size(&[]));
                flow.kind = FlowKind::Group;
                flow.width = s.width;
                flow.height = s.height;
            } else {
                flow.parent_id = group_of(w, node).map(str::to_string);
            }
            flow
        })
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    let edges = w
        .edges
        .iter()
        .map(|e| {
            let mut edge = make_edge(&e.from, &e.to, e.handle.clone(), e.loop_spec.clone());
            let mut id = edge.id.clone();
            let mut n = 2;
            while seen.contains(&id) {
                id = format!("{}#{n}", edge.id);
                n += 1;
            }
            seen.insert(id.clone());
            edge.id = id;
            edge
        })
        .collect();
    Flow {
        nodes: parents_first(nodes),
        edges,
    }
}

/// Rounds to one decimal like JS `Math.round(n * 10) / 10`.
fn round1(n: f32) -> f64 {
    ((n as f64) * 10.0 + 0.5).floor() / 10.0
}

pub fn from_flow(base: &Workflow, nodes: &[FlowNode], edges: &[FlowEdge]) -> Workflow {
    let flow_by_id: HashMap<&str, &FlowNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let order: Vec<&str> = base
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .filter(|id| flow_by_id.contains_key(id))
        .chain(
            nodes
                .iter()
                .map(|n| n.id.as_str())
                .filter(|id| !base.nodes.iter().any(|b| b.id == *id)),
        )
        .collect();
    let out_nodes = order
        .into_iter()
        .map(|id| {
            let flow = flow_by_id[id];
            let mut node = flow.data.node.clone();
            node.parent = flow.parent_id.clone();
            node.ui = Some(Ui {
                x: round1(flow.position.x),
                y: round1(flow.position.y),
            });
            node
        })
        .collect();
    let out_edges = edges
        .iter()
        .map(|e| {
            let handle = e
                .source_handle
                .as_deref()
                .filter(|h| !h.is_empty())
                .map(Handle::named);
            Edge {
                from: e.source.clone(),
                to: e.target.clone(),
                handle: if e.loop_spec.is_some() { None } else { handle },
                loop_spec: e.loop_spec.clone(),
            }
        })
        .collect();
    // What is saved is laid out left to right, so the file says so.
    Workflow {
        nodes: out_nodes,
        edges: out_edges,
        ui: Some(WorkflowUi::lr()),
        ..base.clone()
    }
}

// --- what may be connected -----------------------------------------------------------------------

/// Whether `to` can be reached from `from` along ordinary (non-loop) edges.
fn reaches(w: &Workflow, from: &str, to: &str) -> bool {
    let mut seen: HashSet<&str> = HashSet::from([from]);
    let mut queue = vec![from];
    while let Some(id) = queue.pop() {
        for e in &w.edges {
            if e.loop_spec.is_some() || e.from != id || seen.contains(e.to.as_str()) {
                continue;
            }
            if e.to == to {
                return true;
            }
            seen.insert(&e.to);
            queue.push(&e.to);
        }
    }
    false
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionKind {
    Edge,
    Loop,
    Invalid,
}

/// What dragging a connection from `from` to `to` makes. A loop is a connection back to an
/// ancestor in the same branch, allowed from a session or command node that has no
/// loop yet; anything else that would break the graph's rules is invalid. `handle` is the If
/// or Switch handle the connection leaves from; either may send two handles to one node.
pub fn connection_kind(
    w: &Workflow,
    from: &str,
    to: &str,
    handle: Option<Handle>,
) -> ConnectionKind {
    let (Some(a), Some(b)) = (w.node(from), w.node(to)) else {
        return ConnectionKind::Invalid;
    };
    // A Switch has an out handle for each case and for the default, and no other.
    if let (NodeKind::Switch { cases, .. }, Some(h)) = (&a.kind, &handle) {
        if h.as_str() != SWITCH_DEFAULT && !cases.iter().any(|c| c == h.as_str()) {
            return ConnectionKind::Invalid;
        }
    }
    if from == to || b.is_trigger() {
        return ConnectionKind::Invalid;
    }
    if a.parent != b.parent {
        return ConnectionKind::Invalid;
    }
    if w.edges
        .iter()
        .any(|e| e.loop_spec.is_none() && e.from == from && e.to == to && e.handle == handle)
    {
        return ConnectionKind::Invalid;
    }
    if reaches(w, to, from) {
        let can_loop = matches!(a.kind, NodeKind::Session { .. } | NodeKind::Command { .. });
        if !can_loop
            || w.edges
                .iter()
                .any(|e| e.loop_spec.is_some() && e.from == from)
        {
            return ConnectionKind::Invalid;
        }
        return ConnectionKind::Loop;
    }
    ConnectionKind::Edge
}

// --- new nodes -------------------------------------------------------------------------------------

fn unique_id(w: &Workflow, base: &str) -> NodeId {
    let taken = |id: &str| w.nodes.iter().any(|n| n.id == id);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}_{n}"))
        .find(|id| !taken(id))
        .unwrap_or_default()
}

/// The first `project` input of the workflow as a node's `project`, or "" when it has none.
fn default_project(w: &Workflow) -> String {
    w.inputs
        .iter()
        .find(|(_, input)| input.ty == InputType::Project)
        .map(|(name, _)| format!("{{{{inputs.{name}}}}}"))
        .unwrap_or_default()
}

pub fn new_node(ty: NodeType, w: &Workflow) -> Node {
    let id = unique_id(w, ty.id_base());
    let kind = match ty {
        NodeType::Schedule => NodeKind::Schedule {
            schedule: Schedule {
                schedule_kind: "weekdays".into(),
                time: "09:00".into(),
                minute: 0,
                day_of_week: 1,
                interval_hours: None,
                interval_from: None,
            },
        },
        NodeType::Manual => NodeKind::Manual,
        NodeType::Session => NodeKind::Session {
            project: default_project(w),
            prompt: String::new(),
            checkout: Checkout::Root,
            output: OrderedMap::new(),
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
        },
        NodeType::Command => NodeKind::Command {
            project: default_project(w),
            command: String::new(),
            cwd: None,
            timeout_minutes: None,
        },
        NodeType::If => NodeKind::If {
            condition: None,
            decide: Some(Decide::Ai),
            question: None,
        },
        NodeType::Switch => NodeKind::Switch {
            value: None,
            cases: vec!["a".into(), "b".into()],
            decide: Some(Decide::Ai),
            question: None,
        },
        NodeType::ForEach => NodeKind::ForEach {
            list: String::new(),
            item: Some("item".into()),
            max_parallel: Some(3.0),
        },
    };
    Node::new(id, kind)
}

/// "web app" -> "web_app": something that can be an input name.
fn input_name(label: &str) -> String {
    let mut name = String::new();
    let mut in_run = false;
    for c in label.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            name.push(c);
            in_run = false;
        } else if !in_run {
            name.push('_');
            in_run = true;
        }
    }
    let name = name.trim_matches('_').to_ascii_lowercase();
    if name.starts_with(|c: char| c.is_ascii_lowercase() || c == '_') {
        name
    } else if name.is_empty() {
        String::new()
    } else {
        format!("p_{name}")
    }
}

/// What `project_input_for` decided: the `{{inputs.<name>}}` a node's `project` becomes, and the
/// workflow's inputs and input values with that project in them.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectInput {
    pub reference: String,
    pub inputs: OrderedMap<WorkflowInput>,
    pub input_values: OrderedMap<InputValue>,
}

/// JS truthiness of an input value.
fn truthy(value: Option<&InputValue>) -> bool {
    match value {
        None => false,
        Some(InputValue::String(s)) => !s.is_empty(),
        Some(InputValue::Number(n)) => *n != 0.0 && !n.is_nan(),
        Some(InputValue::Bool(b)) => *b,
    }
}

/// Whether `text` has `inputs.<key>` not followed by a name character. `key` is a plain name
/// (letters, digits, underscore), so a substring search is the regex the React code builds.
fn mentions_input(text: &str, key: &str) -> bool {
    let needle = format!("inputs.{key}");
    text.match_indices(&needle).any(|(at, _)| {
        !text[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The `project` input that stands for a project: the one already holding it, or a new one
/// (named "project" the first time, after the project's name later). A node's `project` is then
/// `{{inputs.<name>}}`, so the definition never holds a path. `current` is the input the step
/// uses now, which takes the project first when it has no value.
pub fn project_input_for(
    w: &Workflow,
    path: &str,
    label: &str,
    current: Option<&str>,
) -> ProjectInput {
    let reuse = |name: &str, value: &str| {
        let held = matches!(w.input_values.get(name), Some(InputValue::String(v)) if v == value);
        let mut input_values = w.input_values.clone();
        if !held {
            input_values.insert(name, InputValue::String(value.to_string()));
        }
        ProjectInput {
            reference: format!("{{{{inputs.{name}}}}}"),
            inputs: w.inputs.clone(),
            input_values,
        }
    };
    let is_project =
        |key: &str| matches!(w.inputs.get(key), Some(input) if input.ty == InputType::Project);
    let unset = |key: &str| is_project(key) && !truthy(w.input_values.get(key));
    let held = w.inputs.keys().find(|key| {
        is_project(key)
            && matches!(w.input_values.get(key), Some(InputValue::String(v)) if v == path)
    });
    if let Some(held) = held {
        return reuse(held, path);
    }
    // An input that is declared but has no value is there to be filled, when it is the step's
    // own or nothing else uses it: binding another step to it would move that step's project.
    let text = format!(
        "{}\n{}",
        serde_json::to_string(&w.nodes).unwrap_or_default(),
        serde_json::to_string(&w.edges).unwrap_or_default()
    );
    let empty = match current {
        Some(current) if !current.is_empty() && unset(current) => Some(current.to_string()),
        _ => w
            .inputs
            .keys()
            .find(|k| unset(k) && !mentions_input(&text, k))
            .cloned(),
    };
    if let Some(empty) = empty {
        return reuse(&empty, path);
    }
    let base = if w.inputs.contains_key("project") {
        Some(input_name(label)).filter(|n| !n.is_empty())
    } else {
        None
    }
    .unwrap_or_else(|| "project".to_string());
    let mut name = base.clone();
    let mut n = 2;
    while w.inputs.contains_key(&name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    let mut inputs = w.inputs.clone();
    inputs.insert(&name, WorkflowInput::new(InputType::Project));
    let mut input_values = w.input_values.clone();
    input_values.insert(&name, InputValue::String(path.to_string()));
    ProjectInput {
        reference: format!("{{{{inputs.{name}}}}}"),
        inputs,
        input_values,
    }
}

// --- groups ------------------------------------------------------------------------------------------

/// Every group sized to wrap its children.
pub fn fit_groups(nodes: &[FlowNode]) -> Vec<FlowNode> {
    nodes
        .iter()
        .map(|n| {
            let mut n = n.clone();
            if n.kind == FlowKind::Group {
                let children: Vec<Point> = nodes
                    .iter()
                    .filter(|c| c.parent_id.as_deref() == Some(n.id.as_str()))
                    .map(|c| c.position)
                    .collect();
                let s = group_size(&children);
                n.width = s.width;
                n.height = s.height;
            }
            n
        })
        .collect()
}

/// `after` (a refit of `before`) with what lies after a group that grew moved along by as much:
/// the nodes to its right when it got wider, and the ones under it in the same column when it
/// got taller, so growing a For each does not run it into what follows. The triggers stay put.
pub fn push_right(before: &[FlowNode], after: &[FlowNode]) -> Vec<FlowNode> {
    let mut out = after.to_vec();
    for g in before.iter().filter(|n| n.kind == FlowKind::Group) {
        let now = after.iter().find(|n| n.id == g.id);
        let wider = now.map_or(0.0, |n| n.width) - g.width;
        let taller = now.map_or(0.0, |n| n.height) - g.height;
        let (right, bottom) = (g.position.x + g.width, g.position.y + g.height);
        for n in out.iter_mut() {
            if n.parent_id.is_some() || n.id == g.id || is_top_trigger(n) {
                continue;
            }
            if wider > 0.0 && n.position.x >= right {
                n.position.x += wider;
            } else if taller > 0.0
                && n.position.y >= bottom
                && n.position.x < right
                && n.position.x + n.width > g.position.x
            {
                n.position.y += taller;
            }
        }
    }
    out
}

/// A node's position on the canvas, a child's being relative to its group.
pub fn absolute(nodes: &[FlowNode], node: &FlowNode) -> Point {
    let parent = node
        .parent_id
        .as_ref()
        .and_then(|id| nodes.iter().find(|n| n.id == *id));
    match parent {
        Some(p) => Point {
            x: p.position.x + node.position.x,
            y: p.position.y + node.position.y,
        },
        None => node.position,
    }
}

/// After a node was dragged: it joins the For each its centre is over, or leaves the one it was
/// in when it is no longer over it. Positions are converted so it stays where it was dropped; a
/// child is kept inside its group's padding, and the groups are refitted.
pub fn settle_drop(nodes: &[FlowNode], id: &str) -> Vec<FlowNode> {
    let Some(node) = nodes.iter().find(|n| n.id == id) else {
        return nodes.to_vec();
    };
    let abs = absolute(nodes, node);
    let centre = Point {
        x: abs.x + NODE_W / 2.0,
        y: abs.y + NODE_H / 2.0,
    };
    let target = if groupable(&node.data.node) {
        nodes.iter().find(|g| {
            g.kind == FlowKind::Group
                && g.id != id
                && centre.x >= g.position.x
                && centre.x <= g.position.x + g.width
                && centre.y >= g.position.y
                && centre.y <= g.position.y + g.height
        })
    } else {
        None
    };
    let mut settled = node.clone();
    match target {
        Some(t) => {
            settled.parent_id = Some(t.id.clone());
            settled.position = Point {
                x: (abs.x - t.position.x).max(GROUP_PAD),
                y: (abs.y - t.position.y).max(GROUP_HEADER),
            };
        }
        None => {
            settled.parent_id = None;
            settled.position = abs;
        }
    }
    let placed = parents_first(
        nodes
            .iter()
            .map(|n| {
                if n.id == id {
                    settled.clone()
                } else {
                    n.clone()
                }
            })
            .collect(),
    );
    stack_triggers(push_right(&placed, &fit_groups(&placed)), Some(id))
}

/// The edges without the ones that touch `ids` and now join nodes of different scopes. A node
/// that joined or left a For each can no longer be wired to nodes on the other side of its
/// frame, and the engine only follows edges between nodes with the same parent.
pub fn drop_crossing_edges(
    nodes: &[FlowNode],
    edges: &[FlowEdge],
    ids: &[NodeId],
) -> Vec<FlowEdge> {
    let parent_of: HashMap<&str, Option<&str>> = nodes
        .iter()
        .map(|n| (n.id.as_str(), n.parent_id.as_deref()))
        .collect();
    let scope = |id: &str| parent_of.get(id).copied().flatten();
    edges
        .iter()
        .filter(|e| {
            !(ids.contains(&e.source) || ids.contains(&e.target))
                || scope(&e.source) == scope(&e.target)
        })
        .cloned()
        .collect()
}

fn without_refs(node: &Node, gone: &HashSet<&str>) -> Node {
    let mut node = node.clone();
    match &mut node.kind {
        NodeKind::Session {
            nest_under,
            checkout,
            ..
        } => {
            // A `sameAs` that names a removed node is left as it is: turning it into a new
            // instance would silently make the run create (and pay for) one more instance. The
            // validator reports the missing node.
            if nest_under.as_deref().is_some_and(|n| gone.contains(n)) {
                *nest_under = None;
            }
            if matches!(checkout, Checkout::WorktreeOf(id) if gone.contains(id.as_str())) {
                *checkout = Checkout::Root;
            }
        }
        NodeKind::Command { cwd, .. } => {
            if matches!(cwd, Some(Cwd::WorktreeOf(id)) if gone.contains(id.as_str())) {
                *cwd = None;
            }
        }
        _ => {}
    }
    node
}

/// The board without some nodes: their edges go, the children of a removed For each become
/// top-level where they were drawn, and nothing is left pointing at a removed session.
pub fn drop_nodes(nodes: &[FlowNode], edges: &[FlowEdge], ids: &[NodeId]) -> Flow {
    let gone: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let kept: Vec<FlowNode> = nodes
        .iter()
        .filter(|n| !gone.contains(n.id.as_str()))
        .map(|n| {
            let mut n = n.clone();
            if n.parent_id.as_deref().is_some_and(|p| gone.contains(p)) {
                n.position = absolute(nodes, &n);
                n.parent_id = None;
            }
            n.data.node = without_refs(&n.data.node, &gone);
            n
        })
        .collect();
    Flow {
        nodes: stack_triggers(fit_groups(&kept), None),
        edges: edges
            .iter()
            .filter(|e| !gone.contains(e.source.as_str()) && !gone.contains(e.target.as_str()))
            .cloned()
            .collect(),
    }
}

/// What a keyboard delete removes. The canvas also lists the children of a deleted For each, and
/// every edge touching them; the children stay (the group's frame goes) and so do their wires.
/// Edges go when they touch a removed node or were selected themselves.
pub fn deletion(nodes: &[FlowNode], edges: &[FlowEdge]) -> Flow {
    let removed: Vec<FlowNode> = nodes
        .iter()
        .filter(|n| {
            n.parent_id.is_none()
                || n.selected
                || !nodes.iter().any(|p| Some(&p.id) == n.parent_id.as_ref())
        })
        .cloned()
        .collect();
    let ids: HashSet<&str> = removed.iter().map(|n| n.id.as_str()).collect();
    let edges = edges
        .iter()
        .filter(|e| {
            ids.contains(e.source.as_str()) || ids.contains(e.target.as_str()) || e.selected
        })
        .cloned()
        .collect();
    Flow {
        nodes: removed,
        edges,
    }
}

/// The messages of the issues that belong to one node, by node id.
pub fn issues_by_node(issues: &[WorkflowIssue]) -> HashMap<NodeId, Vec<String>> {
    let mut out: HashMap<NodeId, Vec<String>> = HashMap::new();
    for issue in issues {
        if let Some(id) = &issue.node_id {
            out.entry(id.clone())
                .or_default()
                .push(issue.message.clone());
        }
    }
    out
}

// --- whole workflows ---------------------------------------------------------------------------------

/// A workflow to start from: a manual trigger, paused. `now` is the creation time in ms.
pub fn blank_workflow(now: u64) -> Workflow {
    let mut trigger = Node::new("start", NodeKind::Manual);
    trigger.title = Some("Manual trigger".into());
    trigger.ui = Some(Ui { x: 0.0, y: 0.0 });
    let mut w = Workflow::new(new_workflow_id(), "New workflow");
    w.nodes = vec![trigger];
    w.created_at = now;
    w
}

/// What makes two versions of a workflow the same to the editor: its definition as the board
/// would draw it (so a workflow with no saved layout equals the same one after being laid out),
/// and not the fields the app manages, which change without the user editing anything. Equal
/// definitions give equal strings.
pub fn definition_key(w: &Workflow) -> String {
    let flow = to_flow(w);
    let flat = from_flow(w, &flow.nodes, &flow.edges);
    let input_values: OrderedMap<&InputValue> = w
        .input_values
        .iter()
        .filter(|(name, _)| w.inputs.contains_key(name))
        .map(|(name, value)| (name.as_str(), value))
        .collect();
    let description = match &w.description {
        Some(d) if !js_trim(d).is_empty() => d.as_str(),
        _ => "",
    };
    let mut value = json!({
        "name": w.name,
        "description": description,
        "enabled": w.enabled,
        "inputs": to_value(&w.inputs),
        "inputValues": to_value(&input_values),
        "nodes": to_value(&flat.nodes),
        "edges": to_value(&flat.edges),
    });
    if let (Some(map), false) = (value.as_object_mut(), w.requires.is_empty()) {
        map.insert("requires".into(), to_value(&w.requires));
    }
    if let Some(map) = value.as_object_mut() {
        if let Some(model) = &w.decision_model {
            map.insert("decisionModel".into(), to_value(model));
        }
        if let Some(report_to) = &w.report_to {
            map.insert("reportTo".into(), to_value(report_to));
        }
        if let Some(template) = w
            .summary_template
            .as_deref()
            .filter(|t| !js_trim(t).is_empty())
        {
            map.insert("summaryTemplate".into(), json!(template));
        }
    }
    stable_json(&value)
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
