//! Deterministic layered layout for flow diagrams. Pure functions only.
//!
//! Algorithm (Sugiyama style; the TypeScript port for the web follows it step
//! by step, so keep the steps and constants in sync):
//!
//! 1. Size every node from its label: `text_width` (0.6 em per character, 1.2
//!    em for wide CJK and emoji), labels wrapped at 26 columns, then padded.
//!    Decisions grow into a diamond, circles into a square circumscribing text.
//! 2. Cycles: an iterative depth-first search in node order; an edge that
//!    points at a node still on the stack is a back edge and is laid out
//!    reversed (its arrowhead still lands on the original target). Self loops
//!    are drawn as a small loop beside the node and take no part in layering.
//! 3. Layering: longest path from the sources (Kahn order, ties by node index).
//! 4. An edge that spans several layers gets a zero-size dummy item in every
//!    layer it crosses, so it owns a lane and never crosses a node.
//! 5. Crossing reduction: initial order is the DFS preorder, then up to 16
//!    alternating barycenter sweeps (stable sort, ties keep the previous
//!    order). The order with the fewest crossings seen wins.
//! 6. Cross-axis coordinates: pack each layer with minimum separations, then 8
//!    rounds of pulling every item toward the mean of its neighbours in the
//!    previous (then next) layer; pushed items keep their order and a layer is
//!    shifted by the mean error, which preserves separations.
//! 7. Ports: edges leaving or entering a node side are spread along it (sorted
//!    by the other end's position). Decisions and circles keep one port.
//! 8. Main-axis coordinates: layers are placed one after another with a gap
//!    wide enough for the widest edge label chip and for one 10px lane per
//!    elbowing edge.
//! 9. Routing: orthogonal. Each edge leaves, runs to its lane in the gap,
//!    changes cross position there and enters. Lanes of edges moving toward
//!    +cross are ordered by descending start, those moving toward -cross by
//!    ascending start, which avoids crossings between neighbours.
//! 10. Edge labels become chips centred on the middle link of their edge,
//!     clamped inside the gap, then nudged along the cross axis until they
//!     overlap no earlier chip and no node.
//! 11. Map (main, cross) to (x, y) for `LR`/`TB`, mirror for `RL`/`BT`, and
//!     translate so the content starts at the padding.
use super::{Direction, FlowGraph, NodeKind};
use std::collections::HashMap;

pub const NODE_FONT: f32 = 14.;
pub const EDGE_FONT: f32 = 12.;
pub const LINE_HEIGHT: f32 = 20.;
const WRAP_COLUMNS: f32 = 26.;
const PAD: f32 = 24.;
const RANK_GAP: f32 = 60.;
const NODE_GAP: f32 = 36.;
const DUMMY_GAP: f32 = 14.;
const LANE_SPACING: f32 = 10.;
const LOOP: f32 = 22.;
const CHIP_H: f32 = 22.;
const CHIP_PAD: f32 = 14.;
const CHIP_MARGIN: f32 = 10.;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    /// Strict overlap: rectangles that only touch do not intersect.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedNode {
    /// Index into `FlowGraph::nodes`.
    pub index: usize,
    pub rect: Rect,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLabel {
    pub rect: Rect,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedEdge {
    /// Index into `FlowGraph::edges`.
    pub index: usize,
    /// Orthogonal route from the source to the target (arrow at the last point).
    pub points: Vec<[f32; 2]>,
    pub label: Option<PlacedLabel>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    pub nodes: Vec<PlacedNode>,
    pub edges: Vec<PlacedEdge>,
}

/// Approximate advance of `text` in the monospace font used by diagrams.
pub fn text_width(text: &str, font: f32) -> f32 {
    text.chars().map(char_cells).sum::<f32>() * 0.6 * font
}

fn char_cells(c: char) -> f32 {
    match c as u32 {
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF => 2.,
        _ => 1.,
    }
}

/// Wrap a label into lines of at most `WRAP_COLUMNS` columns; explicit `\n`
/// always breaks. Words longer than a line are split.
pub fn wrap_label(label: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in label.split('\n') {
        let mut line = String::new();
        let mut cells = 0.;
        for word in paragraph.split_whitespace() {
            let word_cells: f32 = word.chars().map(char_cells).sum();
            if !line.is_empty() && cells + 1. + word_cells > WRAP_COLUMNS {
                lines.push(std::mem::take(&mut line));
                cells = 0.;
            }
            if word_cells > WRAP_COLUMNS {
                for c in word.chars() {
                    let w = char_cells(c);
                    if cells + w > WRAP_COLUMNS {
                        lines.push(std::mem::take(&mut line));
                        cells = 0.;
                    }
                    line.push(c);
                    cells += w;
                }
                continue;
            }
            if !line.is_empty() {
                line.push(' ');
                cells += 1.;
            }
            line.push_str(word);
            cells += word_cells;
        }
        lines.push(line);
    }
    lines
}

struct Sized {
    w: f32,
    h: f32,
    lines: Vec<String>,
}

fn node_size(label: &str, kind: NodeKind) -> Sized {
    let lines = wrap_label(label);
    let tw = lines
        .iter()
        .map(|l| text_width(l, NODE_FONT))
        .fold(0., f32::max);
    let th = lines.len() as f32 * LINE_HEIGHT;
    let (w, h) = match kind {
        NodeKind::Box | NodeKind::Round => ((tw + 32.).max(72.), (th + 20.).max(40.)),
        NodeKind::Start | NodeKind::End => ((tw + 40.).max(80.), (th + 20.).max(40.)),
        NodeKind::Decision => ((1.7 * tw + 40.).max(96.), (2. * th + 16.).max(56.)),
        NodeKind::Circle => {
            let d = (tw + 28.).max(th + 28.).max(56.);
            (d, d)
        }
    };
    Sized { w, h, lines }
}

struct Item {
    real: Option<usize>,
    layer: usize,
    main: f32,
    cross: f32,
    pos: f32,
    m0: f32,
}

#[derive(Clone, Copy)]
struct Link {
    u: usize,
    v: usize,
}

struct Chain {
    edge: usize,
    reversed: bool,
    links: Vec<usize>,
}

struct Chip {
    main: f32,
    cross: f32,
    text: String,
}

fn chip_of(label: &str, horizontal: bool) -> Chip {
    let w = text_width(label, EDGE_FONT) + CHIP_PAD;
    if horizontal {
        Chip {
            main: w,
            cross: CHIP_H,
            text: label.into(),
        }
    } else {
        Chip {
            main: CHIP_H,
            cross: w,
            text: label.into(),
        }
    }
}

pub fn layout(graph: &FlowGraph) -> Result<Layout, String> {
    graph.validate()?;
    let horizontal = graph.direction.horizontal();
    let n = graph.nodes.len();
    let index: HashMap<&str, usize> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, node)| (node.id.as_str(), i))
        .collect();
    let ends: Vec<(usize, usize)> = graph
        .edges
        .iter()
        .map(|e| (index[e.from.as_str()], index[e.to.as_str()]))
        .collect();
    let sized: Vec<Sized> = graph
        .nodes
        .iter()
        .map(|node| node_size(&node.label, node.kind))
        .collect();

    // 2. Break cycles.
    let mut adjacency: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (e, &(a, b)) in ends.iter().enumerate() {
        adjacency[a].push((e, b));
    }
    let mut state = vec![0u8; n];
    let mut reversed = vec![false; ends.len()];
    for start in 0..n {
        if state[start] != 0 {
            continue;
        }
        state[start] = 1;
        let mut stack = vec![(start, 0usize)];
        while let Some(&(u, i)) = stack.last() {
            if i < adjacency[u].len() {
                stack.last_mut().unwrap().1 += 1;
                let (e, v) = adjacency[u][i];
                if v == u {
                    continue;
                }
                match state[v] {
                    0 => {
                        state[v] = 1;
                        stack.push((v, 0));
                    }
                    1 => reversed[e] = true,
                    _ => {}
                }
            } else {
                state[u] = 2;
                stack.pop();
            }
        }
    }
    let oriented: Vec<Option<(usize, usize)>> = ends
        .iter()
        .enumerate()
        .map(|(e, &(a, b))| {
            if a == b {
                None
            } else if reversed[e] {
                Some((b, a))
            } else {
                Some((a, b))
            }
        })
        .collect();

    // 3. Longest-path layering.
    let mut indegree = vec![0usize; n];
    let mut forward: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b) in oriented.iter().flatten() {
        indegree[b] += 1;
        forward[a].push(b);
    }
    let mut layer_of = vec![0usize; n];
    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&i| indegree[i] == 0).collect();
    while let Some(u) = queue.pop_front() {
        for &v in &forward[u] {
            layer_of[v] = layer_of[v].max(layer_of[u] + 1);
            indegree[v] -= 1;
            if indegree[v] == 0 {
                queue.push_back(v);
            }
        }
    }
    let layer_count = layer_of.iter().copied().max().unwrap_or(0) + 1;

    // 4. Items, chains and links.
    let mut items: Vec<Item> = (0..n)
        .map(|i| Item {
            real: Some(i),
            layer: layer_of[i],
            main: if horizontal { sized[i].w } else { sized[i].h },
            cross: if horizontal { sized[i].h } else { sized[i].w },
            pos: 0.,
            m0: 0.,
        })
        .collect();
    let mut links: Vec<Link> = Vec::new();
    let mut chains: Vec<Chain> = Vec::new();
    for (e, pair) in oriented.iter().enumerate() {
        let Some(&(a, b)) = pair.as_ref() else {
            continue;
        };
        let mut chain_links = Vec::new();
        let mut prev = a;
        for layer in layer_of[a] + 1..layer_of[b] {
            items.push(Item {
                real: None,
                layer,
                main: 0.,
                cross: 0.,
                pos: 0.,
                m0: 0.,
            });
            let dummy = items.len() - 1;
            chain_links.push(links.len());
            links.push(Link { u: prev, v: dummy });
            prev = dummy;
        }
        chain_links.push(links.len());
        links.push(Link { u: prev, v: b });
        chains.push(Chain {
            edge: e,
            reversed: reversed[e],
            links: chain_links,
        });
    }
    let count = items.len();
    let mut up: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut down: Vec<Vec<usize>> = vec![Vec::new(); count];
    for link in &links {
        down[link.u].push(link.v);
        up[link.v].push(link.u);
    }

    // 5. Order inside layers.
    let mut rank = vec![usize::MAX; count];
    let mut visited = 0;
    for root in 0..count {
        if !up[root].is_empty() || rank[root] != usize::MAX {
            continue;
        }
        let mut stack = vec![root];
        while let Some(u) = stack.pop() {
            if rank[u] != usize::MAX {
                continue;
            }
            rank[u] = visited;
            visited += 1;
            for &v in down[u].iter().rev() {
                stack.push(v);
            }
        }
    }
    let mut layers: Vec<Vec<usize>> = vec![Vec::new(); layer_count];
    for (i, item) in items.iter().enumerate() {
        layers[item.layer].push(i);
    }
    for layer in &mut layers {
        layer.sort_by_key(|&i| (rank[i], i));
    }
    let crossings = |layers: &[Vec<usize>]| -> usize {
        let mut place = vec![0usize; count];
        for layer in layers {
            for (p, &i) in layer.iter().enumerate() {
                place[i] = p;
            }
        }
        let mut total = 0;
        for layer in layers.iter().take(layers.len().saturating_sub(1)) {
            let pairs: Vec<(usize, usize)> = layer
                .iter()
                .flat_map(|&u| down[u].iter().map(move |&v| (u, v)))
                .map(|(u, v)| (place[u], place[v]))
                .collect();
            for (i, a) in pairs.iter().enumerate() {
                for b in &pairs[i + 1..] {
                    if (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1) {
                        total += 1;
                    }
                }
            }
        }
        total
    };
    let mut best = layers.clone();
    let mut best_crossings = crossings(&layers);
    for round in 0..16 {
        if best_crossings == 0 {
            break;
        }
        let order: Vec<usize> = if round % 2 == 0 {
            (1..layer_count).collect()
        } else {
            (0..layer_count - 1).rev().collect()
        };
        for l in order {
            let neighbours = if round % 2 == 0 { &up } else { &down };
            let mut place = vec![0f32; count];
            for layer in &layers {
                for (p, &i) in layer.iter().enumerate() {
                    place[i] = p as f32;
                }
            }
            let mut keyed: Vec<(f32, usize)> = layers[l]
                .iter()
                .map(|&i| {
                    let nb = &neighbours[i];
                    let key = if nb.is_empty() {
                        place[i]
                    } else {
                        nb.iter().map(|&j| place[j]).sum::<f32>() / nb.len() as f32
                    };
                    (key, i)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            layers[l] = keyed.into_iter().map(|(_, i)| i).collect();
        }
        let c = crossings(&layers);
        if c < best_crossings {
            best_crossings = c;
            best = layers.clone();
        }
    }
    let layers = best;

    // 6. Cross-axis coordinates.
    let separation = |a: usize, b: usize, items: &[Item]| -> f32 {
        let half = (items[a].cross + items[b].cross) / 2.;
        half + if items[a].real.is_some() && items[b].real.is_some() {
            NODE_GAP
        } else {
            DUMMY_GAP
        }
    };
    for layer in &layers {
        let mut at = 0.;
        for (k, &i) in layer.iter().enumerate() {
            if k > 0 {
                at += separation(layer[k - 1], i, &items);
            }
            items[i].pos = at;
        }
        let mid = at / 2.;
        for &i in layer {
            items[i].pos -= mid;
        }
    }
    for round in 0..16 {
        let downward = round % 2 == 0;
        let order: Vec<usize> = if downward {
            (1..layer_count).collect()
        } else {
            (0..layer_count - 1).rev().collect()
        };
        for l in order {
            let layer = &layers[l];
            let desired: Vec<f32> = layer
                .iter()
                .map(|&i| {
                    let nb = if downward { &up[i] } else { &down[i] };
                    if nb.is_empty() {
                        items[i].pos
                    } else {
                        nb.iter().map(|&j| items[j].pos).sum::<f32>() / nb.len() as f32
                    }
                })
                .collect();
            let mut placed = vec![0f32; layer.len()];
            for k in 0..layer.len() {
                placed[k] = if k == 0 {
                    desired[0]
                } else {
                    desired[k].max(placed[k - 1] + separation(layer[k - 1], layer[k], &items))
                };
            }
            let shift =
                desired.iter().zip(&placed).map(|(d, p)| d - p).sum::<f32>() / layer.len() as f32;
            for (k, &i) in layer.iter().enumerate() {
                items[i].pos = placed[k] + shift;
            }
        }
    }

    // 7. Ports.
    let mut out_offset = vec![0f32; links.len()];
    let mut in_offset = vec![0f32; links.len()];
    let spreads = |kind: NodeKind| !matches!(kind, NodeKind::Decision | NodeKind::Circle);
    for i in 0..n {
        if !spreads(graph.nodes[i].kind) {
            continue;
        }
        let mut leaving: Vec<usize> = (0..links.len()).filter(|&k| links[k].u == i).collect();
        leaving.sort_by(|&a, &b| {
            items[links[a].v]
                .pos
                .partial_cmp(&items[links[b].v].pos)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut entering: Vec<usize> = (0..links.len()).filter(|&k| links[k].v == i).collect();
        entering.sort_by(|&a, &b| {
            items[links[a].u]
                .pos
                .partial_cmp(&items[links[b].u].pos)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for (group, offsets) in [(leaving, &mut out_offset), (entering, &mut in_offset)] {
            let k = group.len();
            let step = ((items[i].cross - 16.) / k.max(1) as f32).clamp(0., 12.);
            for (rank, &link) in group.iter().enumerate() {
                offsets[link] = (rank as f32 - (k as f32 - 1.) / 2.) * step;
            }
        }
    }
    let start_of = |k: usize, items: &[Item]| items[links[k].u].pos + out_offset[k];
    let end_of = |k: usize, items: &[Item]| items[links[k].v].pos + in_offset[k];
    let needs_lane = |k: usize, items: &[Item]| (start_of(k, items) - end_of(k, items)).abs() > 1.5;

    // 8. Gaps and main coordinates.
    let mut lanes_in_gap = vec![0usize; layer_count];
    for k in 0..links.len() {
        if needs_lane(k, &items) {
            lanes_in_gap[items[links[k].u].layer] += 1;
        }
    }
    let chips: Vec<Option<Chip>> = graph
        .edges
        .iter()
        .map(|e| {
            e.label
                .as_deref()
                .filter(|l| !l.trim().is_empty())
                .map(|l| chip_of(l, horizontal))
        })
        .collect();
    let mut gap = vec![RANK_GAP; layer_count];
    for l in 0..layer_count {
        gap[l] = gap[l].max((lanes_in_gap[l] + 1) as f32 * LANE_SPACING + 20.);
    }
    let mut label_link = vec![usize::MAX; chains.len()];
    for (c, chain) in chains.iter().enumerate() {
        let k = chain.links[(chain.links.len() - 1) / 2];
        label_link[c] = k;
        if let Some(chip) = &chips[chain.edge] {
            let l = items[links[k].u].layer;
            gap[l] = gap[l].max(chip.main + 2. * CHIP_MARGIN);
        }
    }
    let mut loops_at = vec![0usize; n];
    let mut loop_slot = vec![0usize; ends.len()];
    for (e, &(a, b)) in ends.iter().enumerate() {
        if a == b {
            loop_slot[e] = loops_at[a];
            loops_at[a] += 1;
            let l = layer_of[a];
            let reach =
                LOOP + 8. * loops_at[a] as f32 + chips[e].as_ref().map_or(0., |c| c.main + 12.);
            gap[l] = gap[l].max(reach + 2. * CHIP_MARGIN);
        }
    }
    let mut layer_main = vec![0f32; layer_count];
    for item in &items {
        layer_main[item.layer] = layer_main[item.layer].max(item.main);
    }
    let mut layer_start = vec![0f32; layer_count];
    for l in 1..layer_count {
        layer_start[l] = layer_start[l - 1] + layer_main[l - 1] + gap[l - 1];
    }
    for item in &mut items {
        item.m0 = layer_start[item.layer] + (layer_main[item.layer] - item.main) / 2.;
    }

    // 9. Routing.
    let mut lane_x = vec![f32::NAN; links.len()];
    for l in 0..layer_count {
        let mut here: Vec<usize> = (0..links.len())
            .filter(|&k| items[links[k].u].layer == l && needs_lane(k, &items))
            .collect();
        let downward = |k: usize| end_of(k, &items) > start_of(k, &items);
        here.sort_by(|&a, &b| {
            let (da, db) = (downward(a), downward(b));
            // Downward links first (largest start first), then upward ones.
            db.cmp(&da).then_with(|| {
                let (sa, sb) = (start_of(a, &items), start_of(b, &items));
                let ord = sa.partial_cmp(&sb).unwrap_or(std::cmp::Ordering::Equal);
                (if da { ord.reverse() } else { ord }).then(a.cmp(&b))
            })
        });
        let g0 = layer_start[l] + layer_main[l];
        let total = here.len() as f32;
        for (rank, &k) in here.iter().enumerate() {
            lane_x[k] = g0 + gap[l] * (rank as f32 + 1.) / (total + 1.);
        }
    }
    let exit_main = |i: usize, items: &[Item]| items[i].m0 + items[i].main;
    let mut routes: Vec<Vec<[f32; 2]>> = vec![Vec::new(); graph.edges.len()];
    for chain in &chains {
        let mut points: Vec<[f32; 2]> = Vec::new();
        for &k in &chain.links {
            let link = links[k];
            let (sm, em) = (exit_main(link.u, &items), items[link.v].m0);
            let (sc, ec) = (start_of(k, &items), end_of(k, &items));
            points.push([sm, sc]);
            if lane_x[k].is_finite() {
                points.push([lane_x[k], sc]);
                points.push([lane_x[k], ec]);
                points.push([em, ec]);
            } else {
                points.push([em, sc]);
            }
        }
        let mut cleaned = dedupe(points);
        if chain.reversed {
            cleaned.reverse();
        }
        routes[chain.edge] = cleaned;
    }
    let mut chip_rects: Vec<(usize, [f32; 4])> = Vec::new(); // edge, [main, cross, size main, size cross]
    let node_boxes: Vec<[f32; 4]> = (0..n)
        .map(|i| {
            [
                items[i].m0,
                items[i].pos - items[i].cross / 2.,
                items[i].main,
                items[i].cross,
            ]
        })
        .collect();
    let overlaps = |a: &[f32; 4], b: &[f32; 4], by: f32| {
        a[0] - by < b[0] + b[2]
            && b[0] - by < a[0] + a[2]
            && a[1] - by < b[1] + b[3]
            && b[1] - by < a[1] + a[3]
    };
    let mut place_chip = |edge: usize, main: f32, cross: f32, chip: &Chip| {
        let step = chip.cross + 3.;
        let mut candidate = [
            main - chip.main / 2.,
            cross - chip.cross / 2.,
            chip.main,
            chip.cross,
        ];
        let mut chosen = candidate;
        'search: for k in 0..12 {
            let sign = if k % 2 == 1 { 1. } else { -1. };
            let shift = sign * step * ((k + 1) / 2) as f32;
            candidate[1] = cross - chip.cross / 2. + shift;
            let blocked = chip_rects.iter().any(|(_, r)| overlaps(&candidate, r, 1.))
                || node_boxes.iter().any(|r| overlaps(&candidate, r, 2.));
            if !blocked {
                chosen = candidate;
                break 'search;
            }
        }
        chip_rects.push((edge, chosen));
    };
    for (c, chain) in chains.iter().enumerate() {
        let Some(chip) = &chips[chain.edge] else {
            continue;
        };
        let k = label_link[c];
        let l = items[links[k].u].layer;
        let g0 = layer_start[l] + layer_main[l];
        let g1 = g0 + gap[l];
        let (sc, ec) = (start_of(k, &items), end_of(k, &items));
        let (main, cross) = if lane_x[k].is_finite() {
            let lo = g0 + chip.main / 2. + 4.;
            let hi = g1 - chip.main / 2. - 4.;
            (lane_x[k].clamp(lo.min(hi), hi.max(lo)), (sc + ec) / 2.)
        } else {
            ((g0 + g1) / 2., sc)
        };
        place_chip(chain.edge, main, cross, chip);
    }
    for (e, &(a, b)) in ends.iter().enumerate() {
        if a != b {
            continue;
        }
        let item = &items[a];
        let slot = loop_slot[e] as f32;
        let reach = LOOP + 8. * slot;
        let quarter = item.cross * 0.3;
        let edge_main = item.m0 + item.main;
        routes[e] = vec![
            [edge_main, item.pos - quarter],
            [edge_main + reach, item.pos - quarter],
            [edge_main + reach, item.pos + quarter],
            [edge_main, item.pos + quarter],
        ];
        if let Some(chip) = &chips[e] {
            place_chip(e, edge_main + reach + chip.main / 2. + 6., item.pos, chip);
        }
    }

    // 11. Map to screen space.
    let to_screen = |m: f32, c: f32| if horizontal { [m, c] } else { [c, m] };
    let rect_of = |b: &[f32; 4]| {
        if horizontal {
            Rect {
                x: b[0],
                y: b[1],
                w: b[2],
                h: b[3],
            }
        } else {
            Rect {
                x: b[1],
                y: b[0],
                w: b[3],
                h: b[2],
            }
        }
    };
    let mut nodes: Vec<PlacedNode> = (0..n)
        .map(|i| PlacedNode {
            index: i,
            rect: rect_of(&node_boxes[i]),
            lines: sized[i].lines.clone(),
        })
        .collect();
    let mut edges: Vec<PlacedEdge> = (0..graph.edges.len())
        .map(|e| PlacedEdge {
            index: e,
            points: routes[e].iter().map(|p| to_screen(p[0], p[1])).collect(),
            label: chip_rects
                .iter()
                .find(|(edge, _)| *edge == e)
                .map(|(_, r)| PlacedLabel {
                    rect: rect_of(r),
                    text: chips[e]
                        .as_ref()
                        .map(|c| c.text.clone())
                        .unwrap_or_default(),
                }),
        })
        .collect();
    // Bounds, mirroring and translation.
    let mut min = [f32::MAX; 2];
    let mut max = [f32::MIN; 2];
    let mut include = |x: f32, y: f32| {
        min = [min[0].min(x), min[1].min(y)];
        max = [max[0].max(x), max[1].max(y)];
    };
    for node in &nodes {
        include(node.rect.x, node.rect.y);
        include(node.rect.right(), node.rect.bottom());
    }
    for edge in &edges {
        for p in &edge.points {
            include(p[0], p[1]);
        }
        if let Some(label) = &edge.label {
            include(label.rect.x, label.rect.y);
            include(label.rect.right(), label.rect.bottom());
        }
    }
    let width = max[0] - min[0] + 2. * PAD;
    let height = max[1] - min[1] + 2. * PAD;
    let flip_x = graph.direction == Direction::RL;
    let flip_y = graph.direction == Direction::BT;
    let map = |x: f32, y: f32| -> [f32; 2] {
        [
            if flip_x { max[0] - x } else { x - min[0] } + PAD,
            if flip_y { max[1] - y } else { y - min[1] } + PAD,
        ]
    };
    let map_rect = |r: &Rect| -> Rect {
        let a = map(r.x, r.y);
        let b = map(r.right(), r.bottom());
        Rect {
            x: a[0].min(b[0]),
            y: a[1].min(b[1]),
            w: (a[0] - b[0]).abs(),
            h: (a[1] - b[1]).abs(),
        }
    };
    for node in &mut nodes {
        node.rect = map_rect(&node.rect);
    }
    for edge in &mut edges {
        for p in &mut edge.points {
            *p = map(p[0], p[1]);
        }
        if let Some(label) = &mut edge.label {
            label.rect = map_rect(&label.rect);
        }
    }
    Ok(Layout {
        width,
        height,
        nodes,
        edges,
    })
}

/// Drop repeated points and merge collinear runs.
fn dedupe(points: Vec<[f32; 2]>) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::new();
    for p in points {
        if out
            .last()
            .is_some_and(|q| (q[0] - p[0]).abs() < 0.01 && (q[1] - p[1]).abs() < 0.01)
        {
            continue;
        }
        while out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let same_x = (a[0] - b[0]).abs() < 0.01 && (b[0] - p[0]).abs() < 0.01;
            let same_y = (a[1] - b[1]).abs() < 0.01 && (b[1] - p[1]).abs() < 0.01;
            if same_x || same_y {
                out.pop();
            } else {
                break;
            }
        }
        out.push(p);
    }
    out
}

/// Round the corners of a polyline: every interior vertex is replaced by a
/// quadratic curve of at most `radius` that never eats more than half of an
/// adjacent segment.
pub fn smooth(points: &[[f32; 2]], radius: f32) -> Vec<[f32; 2]> {
    if points.len() < 3 || radius <= 0. {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for w in 1..points.len() - 1 {
        let (a, b, c) = (points[w - 1], points[w], points[w + 1]);
        let (la, lc) = (dist(a, b), dist(b, c));
        let r = radius.min(la / 2.).min(lc / 2.);
        if r < 0.5 {
            out.push(b);
            continue;
        }
        let p = [b[0] + (a[0] - b[0]) * r / la, b[1] + (a[1] - b[1]) * r / la];
        let q = [b[0] + (c[0] - b[0]) * r / lc, b[1] + (c[1] - b[1]) * r / lc];
        for step in 0..=6 {
            let t = step as f32 / 6.;
            let s = 1. - t;
            out.push([
                s * s * p[0] + 2. * s * t * b[0] + t * t * q[0],
                s * s * p[1] + 2. * s * t * b[1] + t * t * q[1],
            ]);
        }
    }
    out.push(points[points.len() - 1]);
    out
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2))
        .sqrt()
        .max(1e-6)
}

/// The three corners of an arrowhead whose tip is `tip`, pointing away from
/// `from` (the previous point of the line).
pub fn arrowhead(tip: [f32; 2], from: [f32; 2], size: f32) -> [[f32; 2]; 3] {
    let d = dist(tip, from);
    let (ux, uy) = ((tip[0] - from[0]) / d, (tip[1] - from[1]) / d);
    let base = [tip[0] - ux * size, tip[1] - uy * size];
    let half = size * 0.45;
    [
        tip,
        [base[0] - uy * half, base[1] + ux * half],
        [base[0] + uy * half, base[1] - ux * half],
    ]
}

/// Split a polyline into dashes of `on` units separated by `off` units.
pub fn dash_polyline(points: &[[f32; 2]], on: f32, off: f32) -> Vec<Vec<[f32; 2]>> {
    let mut dashes = Vec::new();
    let mut current: Vec<[f32; 2]> = Vec::new();
    let mut drawing = true;
    let mut left = on;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = dist(a, b);
        let mut at = 0.;
        if drawing && current.is_empty() {
            current.push(a);
        }
        while length - at > left {
            at += left;
            let p = [
                a[0] + (b[0] - a[0]) * at / length,
                a[1] + (b[1] - a[1]) * at / length,
            ];
            if drawing {
                current.push(p);
                dashes.push(std::mem::take(&mut current));
                left = off;
            } else {
                current.push(p);
                left = on;
            }
            drawing = !drawing;
        }
        left -= length - at;
        if drawing {
            current.push(b);
        }
    }
    if current.len() > 1 {
        dashes.push(current);
    }
    dashes
}
