//! What the canvas draws, laid out in graph space: the cards and frames at their place on the
//! canvas, the edges already routed from handle to handle, and the strips (lane titles, item
//! tabs, round selector) that sit above a run's lanes. Built from the pure board and run models,
//! so the board editor draws from the same thing.
use super::geometry::{Rect, Seg, Side, arrow_head, bounds_of, rounded, step_points};
use monocode_core::mono::workflows::{
    board_edit::{shared_by_all_triggers, trigger_ids},
    board_model::{
        EdgeKind, FlowEdge, FlowKind, FlowNode, Point, absolute, out_handles, trigger_frame,
    },
    model::{LoopSpec, Node},
    ordered::js_number_text,
    run_graph_model::{RunEdgeState, RunNodeStatus},
    template::js_trim,
};

/// How far a plain edge runs clear of a card before it turns, and how round its corners are
/// (React Flow's smooth-step defaults).
const STEP_OFFSET: f32 = 20.;
const STEP_RADIUS: f32 = 5.;
/// The loop edge's run beside the cards and its corner radius.
const LOOP_OFFSET: f32 = 26.;
const LOOP_RADIUS: f32 = 12.;
/// Arrowhead of the edges: the closed 12 x 12 marker at the edge's stroke width.
const ARROW_LENGTH: f32 = 5.;
const ARROW_HALF: f32 = 3.5;
/// Where the true and false outs of an If leave its right side (a share of the card's height),
/// and where the loop handles sit on the bottom (a share of its width).
pub(super) const TRUE_AT: f32 = 0.28;
pub(super) const FALSE_AT: f32 = 0.72;
/// Where the first and last of three or more outs (a Switch) leave its right side.
const FIRST_OF_MANY_AT: f32 = 0.12;
const LAST_OF_MANY_AT: f32 = 0.88;
pub(super) const LOOP_OUT_AT: f32 = 0.7;
pub(super) const LOOP_IN_AT: f32 = 0.3;
/// The loop label is a pill this tall, and about this wide per character.
pub(in crate::monos) const LABEL_H: f32 = 16.;
const LABEL_CHAR: f32 = 6.2;
const LABEL_PAD: f32 = 12.;

/// Where, as a share of the card's height, the out number `index` of `count` leaves its right: a
/// lone one in the middle, two at the places of an If's true and false, more spread evenly.
pub(super) fn out_at(index: usize, count: usize) -> f32 {
    match count {
        0 | 1 => 0.5,
        2 => [TRUE_AT, FALSE_AT][index.min(1)],
        _ => {
            let step = (LAST_OF_MANY_AT - FIRST_OF_MANY_AT) / (count - 1) as f32;
            FIRST_OF_MANY_AT + step * index.min(count - 1) as f32
        }
    }
}

/// Where the out of `node` named `handle` leaves its right side, as a share of the card's height.
fn out_at_of(node: &Node, handle: Option<&str>) -> f32 {
    let outs = out_handles(node);
    handle
        .and_then(|name| outs.iter().position(|o| o == name))
        .map_or(0.5, |index| out_at(index, outs.len()))
}

/// A card or a For each frame at its place on the canvas.
#[derive(Clone, Debug)]
pub(in crate::monos) struct SceneNode {
    /// The flow node; its `position` stays relative to its frame, `rect` is the canvas place.
    pub flow: FlowNode,
    pub rect: Rect,
    /// The key that picks it in a run (`<nodeId>|<branch>`).
    pub key: Option<String>,
    /// A card of a lane shown alone while other items wait behind it: drawn with cards stacked
    /// behind it.
    pub stacked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct EdgeLabel {
    /// The left edge of the pill, at its vertical middle: centred on the run of the loop line
    /// under the cards, so it never covers a card.
    pub at: Point,
    pub text: String,
    /// The text with the whole condition.
    pub full: String,
    /// An estimate of the width, for the bounds only: the pill itself follows its text.
    pub width: f32,
}

/// An edge as drawn: its path on the canvas, its arrowhead and the state of the run.
#[derive(Clone, Debug)]
pub(in crate::monos) struct RoutedEdge {
    pub id: String,
    pub is_loop: bool,
    pub state: Option<RunEdgeState>,
    pub segs: Vec<Seg>,
    pub arrow: Option<[Point; 3]>,
    pub label: Option<EdgeLabel>,
    /// The edge picked on the board.
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct TabItem {
    pub index: usize,
    pub title: String,
    pub status: Option<RunNodeStatus>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) enum BarKind {
    /// The title above one lane.
    Lane {
        title: String,
        status: RunNodeStatus,
    },
    /// The item tabs of a For each shown one lane at a time.
    Tabs {
        group: String,
        items: Vec<TabItem>,
        selected: usize,
    },
    /// The round selector of a For each a loop ran again.
    Rounds {
        group: String,
        rounds: Vec<u32>,
        selected: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::monos) struct Bar {
    pub id: String,
    pub rect: Rect,
    pub kind: BarKind,
}

#[derive(Clone, Debug, Default)]
pub(in crate::monos) struct Scene {
    /// Frames first, then cards, in the order they are drawn.
    pub nodes: Vec<SceneNode>,
    pub edges: Vec<RoutedEdge>,
    pub bars: Vec<Bar>,
    /// The frame the triggers sit in, drawn behind them.
    pub trigger_frame: Option<Rect>,
    /// A new key means a new picture: the canvas fits its view again.
    pub fit_key: String,
}

/// The pill on a loop edge, "\u{2264} 3 rounds". With `full` it also names who decides: "AI
/// decides \u{b7} max 3", or "If <condition> \u{b7} max 3" (a long condition is cut unless `full`).
pub(in crate::monos) fn loop_label(spec: &LoopSpec, full: bool) -> String {
    if !full {
        let rounds = js_number_text(spec.max_rounds);
        let unit = if rounds == "1" { "round" } else { "rounds" };
        return format!("\u{2264} {rounds} {unit}");
    }
    let condition = js_trim(&spec.decide);
    let who = if monocode_core::mono::workflows::parse::is_ai_decide(&spec.decide) {
        "AI decides".to_string()
    } else {
        format!("If {condition}")
    };
    format!("{who} \u{b7} max {}", js_number_text(spec.max_rounds))
}

/// Where a For each takes and gives its lines: the middle of its frame.
pub(super) fn group_anchor(rect: &Rect) -> f32 {
    rect.y + rect.h / 2.
}

fn route(edge: &FlowEdge, source: &SceneNode, target: &SceneNode) -> RoutedEdge {
    let (s, t) = (source.rect, target.rect);
    let is_loop = edge.kind == EdgeKind::Loop;
    let (from, to, offset, radius, a, b) = if is_loop {
        (
            Side::Bottom,
            Side::Bottom,
            LOOP_OFFSET,
            LOOP_RADIUS,
            Point {
                x: s.x + s.w * LOOP_OUT_AT,
                y: s.bottom(),
            },
            Point {
                x: t.x + t.w * LOOP_IN_AT,
                y: t.bottom(),
            },
        )
    } else {
        let y = |n: &SceneNode, at: f32| {
            if n.flow.kind == FlowKind::Group {
                group_anchor(&n.rect)
            } else {
                n.rect.y + n.rect.h * at
            }
        };
        let out = out_at_of(&source.flow.data.node, edge.source_handle.as_deref());
        (
            Side::Right,
            Side::Left,
            STEP_OFFSET,
            STEP_RADIUS,
            Point {
                x: s.right(),
                y: y(source, out),
            },
            Point {
                x: t.x,
                y: y(target, 0.5),
            },
        )
    };
    let path = step_points(a, from, b, to, offset);
    let label = is_loop
        .then_some(edge.loop_spec.as_ref())
        .flatten()
        .map(|spec| {
            let text = loop_label(spec, false);
            let width = text.chars().count() as f32 * LABEL_CHAR + LABEL_PAD;
            EdgeLabel {
                at: Point {
                    x: path.label.x - width / 2.,
                    y: path.line,
                },
                text,
                full: loop_label(spec, true),
                width,
            }
        });
    RoutedEdge {
        id: edge.id.clone(),
        is_loop,
        state: edge.run,
        segs: rounded(&path.points, radius),
        arrow: arrow_head(&path.points, ARROW_LENGTH, ARROW_HALF),
        label,
        selected: edge.selected,
    }
}

/// How far along a run an edge is, for choosing which of several edges a merged one shows.
fn progress(state: Option<RunEdgeState>) -> u8 {
    match state {
        Some(RunEdgeState::Taken) => 3,
        Some(RunEdgeState::Active) => 2,
        Some(RunEdgeState::Idle) => 1,
        None => 0,
    }
}

/// The edges of the triggers, split in two: one merged edge from the trigger frame to each node
/// every trigger leads to (usually the first step), and, apart, the edges to a node only some of
/// the triggers lead to, which leave their own trigger. A merged edge keeps the id of the first
/// edge it stands for.
fn from_frame(edges: &[FlowEdge], nodes: &[FlowNode]) -> (Vec<FlowEdge>, Vec<FlowEdge>) {
    let triggers = trigger_ids(nodes);
    let from_trigger: Vec<&FlowEdge> = edges
        .iter()
        .filter(|e| triggers.contains(&e.source.as_str()))
        .collect();
    let shared = |target: &str| shared_by_all_triggers(nodes, edges, target);
    let mut merged: Vec<FlowEdge> = Vec::new();
    let mut own: Vec<FlowEdge> = Vec::new();
    for &e in &from_trigger {
        if !shared(&e.target) {
            own.push(e.clone());
            continue;
        }
        match merged.iter_mut().find(|m| m.target == e.target) {
            Some(m) => {
                m.selected |= e.selected;
                if progress(e.run) > progress(m.run) {
                    m.run = e.run;
                }
            }
            None => merged.push(e.clone()),
        }
    }
    (merged, own)
}

impl Scene {
    /// Lays out `nodes` (a child's position is relative to its frame) and routes `edges`.
    pub(in crate::monos) fn build(
        nodes: &[FlowNode],
        edges: &[FlowEdge],
        bars: Vec<Bar>,
        fit_key: String,
    ) -> Scene {
        let mut placed: Vec<SceneNode> = nodes
            .iter()
            .map(|n| {
                let at = absolute(nodes, n);
                SceneNode {
                    rect: Rect::new(at.x, at.y, n.width, n.height),
                    key: n.data.run.as_ref().map(|r| r.node_run_key.clone()),
                    stacked: false,
                    flow: n.clone(),
                }
            })
            .collect();
        // Frames under the cards, so a line over a frame is still seen above its fill.
        placed.sort_by_key(|n| n.flow.kind != FlowKind::Group);
        let frame =
            trigger_frame(nodes).map(|(at, size)| Rect::new(at.x, at.y, size.width, size.height));
        let is_trigger = |id: &str| {
            placed.iter().any(|n| {
                n.flow.id == id && n.flow.parent_id.is_none() && n.flow.data.node.is_trigger()
            })
        };
        let route_between = |e: &FlowEdge, source: Option<&SceneNode>| {
            let source = source?;
            let target = placed.iter().find(|n| n.flow.id == e.target)?;
            Some(route(e, source, target))
        };
        let mut routed: Vec<RoutedEdge> = edges
            .iter()
            .filter(|e| !is_trigger(&e.source))
            .filter_map(|e| route_between(e, placed.iter().find(|n| n.flow.id == e.source)))
            .collect();
        // One edge leaves the trigger frame for what every trigger leads to; the others leave
        // their own trigger.
        if let Some(frame) = frame {
            let (merged, own) = from_frame(edges, nodes);
            for e in &own {
                routed.extend(route_between(
                    e,
                    placed.iter().find(|n| n.flow.id == e.source),
                ));
            }
            for e in merged {
                let Some(mut stand_in) = placed.iter().find(|n| n.flow.id == e.source).cloned()
                else {
                    continue;
                };
                stand_in.rect = frame;
                routed.extend(route_between(&e, Some(&stand_in)));
            }
        }
        Scene {
            nodes: placed,
            edges: routed,
            bars,
            trigger_frame: frame,
            fit_key,
        }
    }

    /// Everything the view fits to: the cards, the bars and the loop labels.
    pub(in crate::monos) fn bounds(&self) -> Option<Rect> {
        let labels = self
            .edges
            .iter()
            .filter_map(|e| e.label.as_ref())
            .map(|l| Rect::new(l.at.x, l.at.y - LABEL_H / 2., l.width, LABEL_H));
        bounds_of(
            self.nodes
                .iter()
                .map(|n| n.rect)
                .chain(self.bars.iter().map(|b| b.rect))
                .chain(self.trigger_frame)
                .chain(labels),
        )
    }
}
