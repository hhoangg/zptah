use super::layout::{arrowhead, dash_polyline, smooth, wrap_label};
use super::mermaid::parse_flowchart;
use super::viewport::*;
use super::*;

fn node(id: &str, label: &str) -> FlowNode {
    FlowNode {
        id: id.into(),
        label: label.into(),
        kind: NodeKind::Box,
        color: None,
    }
}

fn edge(from: &str, to: &str, label: Option<&str>) -> FlowEdge {
    FlowEdge {
        from: from.into(),
        to: to.into(),
        label: label.map(str::to_owned),
        dashed: false,
        arrow: true,
    }
}

fn graph(direction: Direction, ids: &[&str], edges: &[(&str, &str, Option<&str>)]) -> FlowGraph {
    FlowGraph {
        direction,
        nodes: ids
            .iter()
            .map(|id| node(id, &format!("Node {id}")))
            .collect(),
        edges: edges.iter().map(|(a, b, l)| edge(a, b, *l)).collect(),
    }
}

/// No two nodes overlap, no label chip touches a node or another chip, and
/// everything stays inside the reported size.
fn assert_clean(graph: &FlowGraph) -> Layout {
    let layout = layout(graph).unwrap();
    for (i, a) in layout.nodes.iter().enumerate() {
        assert!(a.rect.x >= 0. && a.rect.y >= 0.);
        assert!(a.rect.right() <= layout.width && a.rect.bottom() <= layout.height);
        for b in &layout.nodes[i + 1..] {
            assert!(
                !a.rect.intersects(&b.rect),
                "nodes {} and {} overlap: {:?} {:?}",
                graph.nodes[a.index].id,
                graph.nodes[b.index].id,
                a.rect,
                b.rect
            );
        }
    }
    let labels: Vec<&PlacedLabel> = layout
        .edges
        .iter()
        .filter_map(|e| e.label.as_ref())
        .collect();
    for (i, a) in labels.iter().enumerate() {
        assert!(a.rect.right() <= layout.width && a.rect.bottom() <= layout.height);
        for node in &layout.nodes {
            assert!(
                !a.rect.intersects(&node.rect),
                "label '{}' covers node {}",
                a.text,
                graph.nodes[node.index].id
            );
        }
        for b in &labels[i + 1..] {
            assert!(
                !a.rect.intersects(&b.rect),
                "labels '{}' and '{}' overlap",
                a.text,
                b.text
            );
        }
    }
    layout
}

fn chain(n: usize) -> FlowGraph {
    let ids: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let edges: Vec<(&str, &str, Option<&str>)> =
        (1..n).map(|i| (refs[i - 1], refs[i], None)).collect();
    graph(Direction::LR, &refs, &edges)
}

#[test]
fn layout_is_deterministic() {
    let g = graph(
        Direction::TB,
        &["a", "b", "c", "d", "e"],
        &[
            ("a", "b", Some("x")),
            ("a", "c", None),
            ("b", "d", None),
            ("c", "d", Some("y")),
            ("d", "a", None),
            ("a", "e", None),
        ],
    );
    let first = layout(&g).unwrap();
    for _ in 0..5 {
        assert_eq!(first, layout(&g).unwrap());
    }
}

#[test]
fn layers_follow_edges_in_every_direction() {
    let g = graph(
        Direction::LR,
        &["a", "b", "c"],
        &[("a", "b", None), ("b", "c", None)],
    );
    let l = assert_clean(&g);
    assert!(l.nodes[0].rect.right() < l.nodes[1].rect.x);
    assert!(l.nodes[1].rect.right() < l.nodes[2].rect.x);
    let mut tb = g.clone();
    tb.direction = Direction::TB;
    let l = assert_clean(&tb);
    assert!(l.nodes[0].rect.bottom() < l.nodes[1].rect.y);
    let mut rl = g.clone();
    rl.direction = Direction::RL;
    let l = assert_clean(&rl);
    assert!(l.nodes[2].rect.right() < l.nodes[0].rect.x);
    let mut bt = g;
    bt.direction = Direction::BT;
    let l = assert_clean(&bt);
    assert!(l.nodes[2].rect.bottom() < l.nodes[0].rect.y);
}

#[test]
fn diamond_with_labels_has_no_overlaps() {
    let g = graph(
        Direction::LR,
        &["a", "b", "c", "d", "e", "f"],
        &[
            ("a", "b", None),
            ("b", "c", Some("safe")),
            ("b", "d", Some("risky")),
            ("d", "e", Some("allow")),
            ("d", "f", Some("deny")),
            ("c", "e", Some("skip a long layer")),
        ],
    );
    assert_clean(&g);
    let mut tb = g;
    tb.direction = Direction::TB;
    assert_clean(&tb);
}

#[test]
fn a_cycle_and_a_self_loop_are_laid_out() {
    let mut g = graph(
        Direction::LR,
        &["a", "b", "c"],
        &[
            ("a", "b", Some("go")),
            ("b", "c", None),
            ("c", "a", Some("retry")),
            ("b", "b", Some("again")),
        ],
    );
    let l = assert_clean(&g);
    // The back edge still ends at its original target: arrowhead on node a.
    let back = &l.edges[2];
    let end = back.points.last().unwrap();
    let a = &l.nodes[0].rect;
    assert!((end[0] - a.right()).abs() < 1. || (end[0] - a.x).abs() < 1.);
    assert!(end[1] >= a.y && end[1] <= a.bottom());
    // The self loop leaves and returns to the same node.
    let own = &l.edges[3].points;
    let b = &l.nodes[1].rect;
    assert!(own.len() >= 3 && own.iter().all(|p| p[1] >= b.y && p[1] <= b.bottom()));
    g.direction = Direction::TB;
    assert_clean(&g);
}

#[test]
fn thirty_nodes_with_cross_links_do_not_overlap() {
    let ids: Vec<String> = (0..30).map(|i| format!("n{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut edges: Vec<(&str, &str, Option<&str>)> = Vec::new();
    for i in 1..30 {
        edges.push((
            refs[(i - 1) / 2],
            refs[i],
            if i % 3 == 0 { Some("yes") } else { None },
        ));
    }
    edges.push((refs[29], refs[3], Some("loop back")));
    edges.push((refs[5], refs[28], None));
    edges.push((refs[0], refs[20], Some("long")));
    edges.push((refs[7], refs[7], None));
    let g = graph(Direction::LR, &refs, &edges);
    let l = assert_clean(&g);
    assert_eq!(l.nodes.len(), 30);
    let mut tb = g;
    tb.direction = Direction::TB;
    assert_clean(&tb);
}

#[test]
fn a_long_chain_and_disconnected_nodes_are_clean() {
    assert_clean(&chain(30));
    let mut g = chain(4);
    g.nodes.push(node(
        "lonely",
        "Alone with a very long label that must wrap onto several lines",
    ));
    g.nodes.push(node("other", "Other"));
    let l = assert_clean(&g);
    assert!(l.nodes[4].lines.len() >= 3);
}

#[test]
fn edges_do_not_pass_through_unrelated_nodes() {
    let g = graph(
        Direction::LR,
        &["a", "b", "c", "d"],
        &[
            ("a", "b", None),
            ("b", "c", None),
            ("c", "d", None),
            ("a", "d", Some("skip")),
        ],
    );
    let l = assert_clean(&g);
    let skip = &l.edges[3].points;
    for pair in skip.windows(2) {
        let (p, q) = (pair[0], pair[1]);
        let bounds = Rect {
            x: p[0].min(q[0]),
            y: p[1].min(q[1]),
            w: (p[0] - q[0]).abs().max(0.01),
            h: (p[1] - q[1]).abs().max(0.01),
        };
        for index in [1, 2] {
            assert!(
                !bounds.intersects(&l.nodes[index].rect),
                "skip edge crosses node {index}"
            );
        }
    }
}

#[test]
fn routes_are_orthogonal_and_end_on_their_nodes() {
    let g = graph(
        Direction::LR,
        &["a", "b", "c"],
        &[("a", "b", None), ("a", "c", None), ("b", "c", None)],
    );
    let l = assert_clean(&g);
    for e in &l.edges {
        for pair in e.points.windows(2) {
            assert!(
                (pair[0][0] - pair[1][0]).abs() < 0.01 || (pair[0][1] - pair[1][1]).abs() < 0.01,
                "diagonal segment {pair:?}"
            );
        }
        let tip = e.points.last().unwrap();
        let target = &l.nodes[if e.index == 0 { 1 } else { 2 }].rect;
        assert!(tip[1] >= target.y - 0.5 && tip[1] <= target.bottom() + 0.5);
        assert!(
            (tip[0] - target.x).abs() < 0.5,
            "arrow tip is on the target side"
        );
    }
}

#[test]
fn validation_reports_clear_errors() {
    let mut g = graph(Direction::LR, &["a", "b"], &[("a", "zzz", None)]);
    let err = g.validate().unwrap_err();
    assert!(
        err.contains("unknown node id 'zzz'") && err.contains("to"),
        "{err}"
    );
    g.edges[0] = edge("nope", "a", None);
    assert!(g.validate().unwrap_err().contains("'nope'"));
    g.edges.clear();
    g.nodes.push(node("a", "again"));
    assert!(g
        .validate()
        .unwrap_err()
        .contains("Duplicate flow node id 'a'"));
    assert!(layout(&g).is_err());
    assert!(FlowGraph::default().validate().is_err());
    let mut many = chain(MAX_FLOW_NODES + 1);
    assert!(many.validate().is_err());
    many.nodes.truncate(MAX_FLOW_NODES);
    many.edges.truncate(MAX_FLOW_NODES - 1);
    assert!(many.validate().is_ok());
}

#[test]
fn direction_and_kind_parsing() {
    assert_eq!(Direction::parse("TD"), Some(Direction::TB));
    assert_eq!(Direction::parse("diagonal"), None);
    assert_eq!(NodeKind::parse("decision"), Some(NodeKind::Decision));
    assert_eq!(NodeKind::parse("hexagon"), None);
}

#[test]
fn mermaid_flowcharts_parse_to_the_shared_model() {
    let g = parse_flowchart(
        "flowchart LR\n  A[Mono request] --> B{Risk?}\n  B -- safe --> C[Auto act]\n  B -->|risky| D([Ask user])\n  D -.-> E((Done))\n  C ==> E\n  E --- A\n",
    )
    .unwrap();
    assert_eq!(g.direction, Direction::LR);
    let nodes: Vec<_> = g
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), n.label.as_str(), n.kind))
        .collect();
    assert_eq!(
        nodes,
        vec![
            ("A", "Mono request", NodeKind::Box),
            ("B", "Risk?", NodeKind::Decision),
            ("C", "Auto act", NodeKind::Box),
            ("D", "Ask user", NodeKind::Start),
            ("E", "Done", NodeKind::Circle),
        ]
    );
    assert_eq!(g.edges.len(), 6);
    assert_eq!(g.edges[1].label.as_deref(), Some("safe"));
    assert_eq!(g.edges[2].label.as_deref(), Some("risky"));
    assert!(g.edges[3].dashed && g.edges[3].arrow);
    assert!(!g.edges[5].arrow);
    assert_clean(&g);
}

#[test]
fn mermaid_chains_semicolons_comments_and_breaks() {
    let g = parse_flowchart(
        "graph TD;\n%% a comment\n%% another\nA[\"Line one<br/>line two\"]-->B-->C; C-->A\n",
    )
    .unwrap();
    assert_eq!(parse_flowchart("graph LR; X-->Y").unwrap().edges.len(), 1);
    assert_eq!(g.direction, Direction::TB);
    assert_eq!(g.nodes[0].label, "Line one\nline two");
    assert_eq!(g.edges.len(), 3);
    assert_eq!(g.nodes.len(), 3);
    // A later definition renames an earlier bare node.
    let g = parse_flowchart("flowchart TB\nA --> B\nB[Bee]\n").unwrap();
    assert_eq!(g.nodes[1].label, "Bee");
}

#[test]
fn mermaid_outside_the_subset_is_rejected_for_fallback() {
    for source in [
        "sequenceDiagram\nA->>B: hi",
        "flowchart LR\nsubgraph s\nA-->B\nend",
        "flowchart LR\nA-->B\nstyle A fill:#f00",
        "flowchart LR\nA-->B & C",
        "flowchart LR\nA:::hot-->B",
        "flowchart LR\nA{{hex}}-->B",
        "flowchart LR\nA-->",
        "flowchart sideways\nA-->B",
        "",
    ] {
        assert!(parse_flowchart(source).is_err(), "accepted {source:?}");
    }
}

#[test]
fn label_anchoring_math() {
    // Defaults keep the original behaviour: (x, y) is the top-left corner.
    assert_eq!(
        label_origin(10., 20., 50., 12., TextAlign::Start, Baseline::Top),
        [10., 20.]
    );
    let line = 12. * 1.4;
    assert_eq!(
        label_origin(10., 20., 50., 12., TextAlign::Center, Baseline::Middle),
        [-15., 20. - line / 2.]
    );
    assert_eq!(
        label_origin(10., 20., 50., 12., TextAlign::End, Baseline::Bottom),
        [-40., 20. - line]
    );
    assert_eq!(
        label_origin(0., 100., 0., 12., TextAlign::Start, Baseline::Alphabetic),
        [0., 100. - ASCENT * 12.]
    );
    // Middle baseline puts the line box symmetric around y.
    let [_, top] = label_origin(0., 18., 0., 12., TextAlign::Start, Baseline::Middle);
    assert!((top + line / 2. - 18.).abs() < 1e-4);
}

#[test]
fn contrast_picks_dark_ink_on_light_fills() {
    assert!(prefers_dark_text(luminance([1., 1., 1.])));
    assert!(prefers_dark_text(luminance([0.98, 0.9, 0.3])));
    assert!(!prefers_dark_text(luminance([0., 0., 0.])));
    assert!(!prefers_dark_text(luminance([0.1, 0.2, 0.5])));
}

#[test]
fn geometry_helpers() {
    let head = arrowhead([10., 0.], [0., 0.], 8.);
    assert_eq!(head[0], [10., 0.]);
    assert!((head[1][0] - 2.).abs() < 1e-4 && (head[2][0] - 2.).abs() < 1e-4);
    assert!(head[1][1] * head[2][1] < 0.);
    let smoothed = smooth(&[[0., 0.], [10., 0.], [10., 10.]], 4.);
    assert_eq!(smoothed[0], [0., 0.]);
    assert_eq!(*smoothed.last().unwrap(), [10., 10.]);
    assert!(smoothed.len() > 3);
    assert!(smoothed.iter().all(|p| p[0] <= 10.001 && p[1] >= -0.001));
    assert_eq!(smooth(&[[0., 0.], [1., 1.]], 4.).len(), 2);
    assert_eq!(wrap_label("a b").len(), 1);
    assert_eq!(wrap_label("one\ntwo").len(), 2);
    assert!(wrap_label(&"x".repeat(60)).len() >= 3);
    assert!((text_width("abcd", 10.) - 24.).abs() < 1e-4);
    assert!((text_width("\u{4e2d}", 10.) - 12.).abs() < 1e-4);
}

#[test]
fn zoom_keeps_the_point_under_the_pointer_fixed() {
    let mut view = Viewport {
        scale: 1.,
        x: 30.,
        y: -10.,
    };
    let before = view.to_content(200., 120.);
    view.zoom_at(1.5, 200., 120.);
    let after = view.to_content(200., 120.);
    assert!((before[0] - after[0]).abs() < 1e-3 && (before[1] - after[1]).abs() < 1e-3);
    assert!((view.scale - 1.5).abs() < 1e-6);
    view.zoom_at(100., 0., 0.);
    assert_eq!(view.scale, MAX_SCALE);
    view.zoom_at(0.0001, 0., 0.);
    assert_eq!(view.scale, MIN_SCALE);
    assert_eq!(Viewport::default().percent(), 100);
}

#[test]
fn fit_centres_and_never_enlarges_past_the_limit() {
    let v = Viewport::fit([1000., 300.], [500., 400.]);
    assert!((v.scale - 0.46).abs() < 1e-4);
    assert!((v.x - 20.).abs() < 1e-3);
    assert!((v.y - (400. - 300. * v.scale) / 2.).abs() < 1e-3);
    let tiny = Viewport::fit([50., 50.], [800., 600.]);
    assert_eq!(tiny.scale, FIT_MAX);
    assert!((tiny.x - (800. - 50. * FIT_MAX) / 2.).abs() < 1e-3);
    let huge = Viewport::fit([100000., 100000.], [400., 300.]);
    assert_eq!(huge.scale, MIN_SCALE);
}

#[test]
fn pan_and_gesture_factors() {
    let mut v = Viewport::default();
    v.pan(5., -7.);
    assert_eq!((v.x, v.y), (5., -7.));
    assert!(wheel_factor(-20.) > 1.05 && wheel_factor(-20.) < 1.15);
    assert!(wheel_factor(20.) < 1.);
    assert_eq!(wheel_factor(-1e6), 2.);
    assert!((pinch_factor(0.1) - 1.1).abs() < 1e-6);
    assert_eq!(pinch_factor(-5.), 0.5);
}

#[test]
fn dashes_cover_the_line_with_gaps() {
    let dashes = dash_polyline(&[[0., 0.], [100., 0.]], 10., 5.);
    assert_eq!(dashes.len(), 7);
    assert_eq!(dashes[0], vec![[0., 0.], [10., 0.]]);
    assert!((dashes[1][0][0] - 15.).abs() < 1e-3);
    let bent = dash_polyline(&[[0., 0.], [8., 0.], [8., 12.]], 10., 5.);
    assert!(
        bent.iter().any(|d| d.len() == 3),
        "a dash can turn the corner"
    );
    assert_eq!(
        dash_polyline(&[[0., 0.], [4., 0.]], 10., 5.),
        vec![vec![[0., 0.], [4., 0.]]]
    );
}
