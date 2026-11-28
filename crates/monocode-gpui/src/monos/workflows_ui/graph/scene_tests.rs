//! The scene the canvas draws: nodes at their place on the canvas and edges routed from handle
//! to handle, built from the pure board and run models.
use super::cards::{port_label, port_label_width};
use super::geometry::{Rect, Seg};
use super::scene::*;
use monocode_core::mono::workflows::{
    board_model::{GAP_X, NODE_H, NODE_W, Point, to_flow},
    model::{LoopSpec, Workflow},
    ordered::Json,
    parse::parse_workflow,
    run_graph_model::RunEdgeState,
};
use serde_json::{Value, json};

fn workflow(nodes: Value, edges: Value) -> Workflow {
    parse_workflow(&Json::from(&json!({
        "id": "wf-aaaaaaaa", "name": "Nightly checks", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 1,
    })))
    .expect("a workflow")
}

fn session(id: &str) -> Value {
    json!({"id": id, "type": "session", "project": "{{inputs.project}}", "prompt": "Do it",
           "checkout": "root", "output": {}})
}

fn scene_of(w: &Workflow) -> Scene {
    let flow = to_flow(w);
    Scene::build(&flow.nodes, &flow.edges, Vec::new(), "fit".into())
}

fn node<'a>(scene: &'a Scene, id: &str) -> &'a SceneNode {
    scene.nodes.iter().find(|n| n.flow.id == id).expect(id)
}

fn first_and_last(edge: &RoutedEdge) -> (Point, Point) {
    let start = match edge.segs.first() {
        Some(Seg::Move(p)) => *p,
        other => panic!("{other:?}"),
    };
    let end = match edge.segs.last() {
        Some(Seg::Line(p)) => *p,
        other => panic!("{other:?}"),
    };
    (start, end)
}

#[test]
fn the_outs_of_a_switch_leave_in_order_by_case_then_default() {
    let w = workflow(
        json!([
            {"id": "route", "type": "switch", "value": "{{a.kind}}", "cases": ["bug", "feature"]},
            session("one"), session("two"), session("three"),
        ]),
        json!([
            {"from": "route", "to": "one", "handle": "bug"},
            {"from": "route", "to": "two", "handle": "feature"},
            {"from": "route", "to": "three", "handle": "default"},
        ]),
    );
    let scene = scene_of(&w);
    let rect = node(&scene, "route").rect;
    let start = |target: &str| {
        let edge = scene
            .edges
            .iter()
            .find(|e| e.id.ends_with(&format!("->{target}")))
            .unwrap();
        first_and_last(edge).0.y - rect.y
    };
    let (bug, feature, default) = (start("one"), start("two"), start("three"));
    assert!(
        bug < feature && feature < default,
        "{bug} {feature} {default}"
    );
    for (got, index) in [(bug, 0), (feature, 1), (default, 2)] {
        assert!((got - rect.h * out_at(index, 3)).abs() < 0.01, "{got}");
    }
}

#[test]
fn a_switch_card_is_as_tall_as_its_cases_need_to_keep_an_ifs_spacing() {
    let spacing_of = |w: &Workflow, id: &str| {
        let scene = scene_of(w);
        let rect = node(&scene, id).rect;
        let outs = monocode_core::mono::workflows::board_model::out_handles(
            &node(&scene, id).flow.data.node,
        )
        .len();
        (rect.h, rect.h * (out_at(1, outs) - out_at(0, outs)))
    };
    let if_w = workflow(
        json!([{"id": "gate", "type": "if", "condition": "{{a.b}}"}]),
        json!([]),
    );
    let (if_h, if_gap) = spacing_of(&if_w, "gate");
    assert_eq!(if_h, NODE_H);
    let mut last = if_h;
    for cases in [2usize, 3, 4, 6, 9] {
        let names: Vec<String> = (0..cases).map(|i| format!("case{i}")).collect();
        let w = workflow(
            json!([{"id": "route", "type": "switch", "value": "{{a.kind}}", "cases": names}]),
            json!([]),
        );
        let (height, gap) = spacing_of(&w, "route");
        assert!(gap + 0.01 >= if_gap, "{cases} cases: {gap} < {if_gap}");
        assert!(height >= last, "{cases} cases: {height} < {last}");
        last = height;
    }
    assert!(last > 2. * NODE_H);
    // One case is two outs, an If's: the same card.
    let one = workflow(
        json!([{"id": "route", "type": "switch", "value": "{{a.kind}}", "cases": ["only"]}]),
        json!([]),
    );
    assert_eq!(spacing_of(&one, "route").0, NODE_H);
}

#[test]
fn out_at_keeps_the_places_of_an_if_and_spreads_more_outs_evenly() {
    assert_eq!(out_at(0, 1), 0.5);
    assert_eq!((out_at(0, 2), out_at(1, 2)), (TRUE_AT, FALSE_AT));
    let five: Vec<f32> = (0..5).map(|i| out_at(i, 5)).collect();
    assert!(five.windows(2).all(|p| p[1] > p[0]));
    assert!(five[0] > 0. && five[4] < 1.);
    let gaps: Vec<f32> = five.windows(2).map(|p| p[1] - p[0]).collect();
    assert!(gaps.iter().all(|g| (g - gaps[0]).abs() < 0.001));
}

#[test]
fn a_port_label_is_cut_to_fit_beside_its_out() {
    assert_eq!(port_label("bug", 40.), "bug");
    let cut = port_label("a very long case name", 40.);
    assert!(
        cut.chars().count() <= 6 && cut.ends_with('\u{2026}'),
        "{cut}"
    );
    // A label sits in the gap between its card and the next column, never over either.
    assert!(port_label_width() > 0.);
    assert!(port_label_width() <= GAP_X - 9.);
}

#[test]
fn an_edge_runs_from_the_right_of_a_card_to_the_left_of_the_next() {
    let w = workflow(
        json!([{"id": "start", "type": "manual"}, session("dev"), {"id": "report", "type": "command", "project": "{{inputs.project}}", "command": "true"}]),
        json!([{"from": "start", "to": "dev"}, {"from": "dev", "to": "report"}]),
    );
    let scene = scene_of(&w);
    // The trigger's edge leaves its frame; the frame is one edge, not one per trigger.
    assert_eq!(scene.edges.len(), 2);
    let (a, b) = (&node(&scene, "dev").rect, &node(&scene, "report").rect);
    let edge = scene
        .edges
        .iter()
        .find(|e| e.id.starts_with("dev"))
        .unwrap();
    let (start, end) = first_and_last(edge);
    assert_eq!(
        start,
        Point {
            x: a.right(),
            y: a.y + NODE_H / 2.
        }
    );
    assert_eq!(
        end,
        Point {
            x: b.x,
            y: b.y + NODE_H / 2.
        }
    );
    // The arrowhead's tip is where the line ends.
    assert_eq!(edge.arrow.unwrap()[0], end);
    assert!(edge.label.is_none());
}

#[test]
fn one_edge_leaves_the_trigger_frame_for_the_first_node() {
    let w = workflow(
        json!([
            {"id": "start", "type": "manual"},
            {"id": "other", "type": "manual"},
            session("dev"),
        ]),
        json!([{"from": "start", "to": "dev"}, {"from": "other", "to": "dev"}]),
    );
    let scene = scene_of(&w);
    let frame = scene.trigger_frame.expect("a frame behind the triggers");
    assert_eq!(scene.edges.len(), 1);
    let (start, end) = first_and_last(&scene.edges[0]);
    let dev = node(&scene, "dev").rect;
    assert_eq!(
        start,
        Point {
            x: frame.right(),
            y: frame.y + frame.h / 2.
        }
    );
    assert_eq!(
        end,
        Point {
            x: dev.x,
            y: dev.y + dev.h / 2.
        }
    );
    // Both triggers sit inside the frame, and the frame is in the bounds.
    for id in ["start", "other"] {
        let r = node(&scene, id).rect;
        assert!(r.x >= frame.x && r.right() <= frame.right(), "{id}");
        assert!(r.y >= frame.y && r.bottom() <= frame.bottom(), "{id}");
    }
    assert!(scene.bounds().unwrap().x <= frame.x);
    // The edge stands for the first of the edges it replaces.
    assert!(scene.edges[0].id.starts_with("start"));
}

#[test]
fn a_workflow_without_triggers_has_no_frame() {
    let w = workflow(json!([session("dev")]), json!([]));
    assert!(scene_of(&w).trigger_frame.is_none());
}

#[test]
fn the_two_outs_of_an_if_leave_from_28_and_72_percent_of_its_height() {
    let w = workflow(
        json!([
            {"id": "start", "type": "manual"},
            {"id": "check", "type": "if", "condition": "{{trigger.x}} == 1"},
            session("yes"), session("no"),
        ]),
        json!([
            {"from": "start", "to": "check"},
            {"from": "check", "to": "yes", "handle": "true"},
            {"from": "check", "to": "no", "handle": "false"},
        ]),
    );
    let scene = scene_of(&w);
    let rect = node(&scene, "check").rect;
    let starts: Vec<f32> = scene
        .edges
        .iter()
        .filter(|e| e.id.starts_with("check"))
        .map(|e| first_and_last(e).0.y - rect.y)
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(
        starts.iter().any(|y| (y - NODE_H * 0.28).abs() < 0.01),
        "{starts:?}"
    );
    assert!(
        starts.iter().any(|y| (y - NODE_H * 0.72).abs() < 0.01),
        "{starts:?}"
    );
}

#[test]
fn a_loop_edge_leaves_the_bottom_of_the_later_card_and_comes_back_into_the_earlier() {
    let w = workflow(
        json!([{"id": "start", "type": "manual"}, session("dev"), session("review")]),
        json!([
            {"from": "start", "to": "dev"},
            {"from": "dev", "to": "review"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "ai"}},
        ]),
    );
    let scene = scene_of(&w);
    let edge = scene
        .edges
        .iter()
        .find(|e| e.is_loop)
        .expect("the loop edge");
    let (dev, review) = (node(&scene, "dev").rect, node(&scene, "review").rect);
    let (start, end) = first_and_last(edge);
    assert_eq!(
        start,
        Point {
            x: review.x + NODE_W * 0.7,
            y: review.bottom()
        }
    );
    assert_eq!(
        end,
        Point {
            x: dev.x + NODE_W * 0.3,
            y: dev.bottom()
        }
    );
    let label = edge.label.as_ref().expect("a label");
    assert_eq!(label.text, "\u{2264} 3 rounds");
    assert_eq!(label.full, "AI decides \u{b7} max 3");
    // Centred half way along the run under the cards, 26 pixels below them.
    let run_y = review.bottom() + 26.;
    assert!((label.at.y - run_y).abs() < 0.01, "{:?}", label.at);
    assert!((label.at.x + label.width / 2. - (start.x + end.x) / 2.).abs() < 0.01);
    assert!(edge.is_loop);
}

#[test]
fn a_compact_loop_edge_hugs_the_cards_and_has_no_label() {
    let w = workflow(
        json!([{"id": "start", "type": "manual"}, session("dev"), session("review")]),
        json!([
            {"from": "start", "to": "dev"},
            {"from": "dev", "to": "review"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "ai"}},
        ]),
    );
    let flow = to_flow(&w);
    let mut edges = flow.edges.clone();
    for e in &mut edges {
        e.compact = true;
    }
    let scene = Scene::build(&flow.nodes, &edges, Vec::new(), "fit".into());
    let edge = scene.edges.iter().find(|e| e.is_loop).unwrap();
    let review = node(&scene, "review").rect;
    let (start, _) = first_and_last(edge);
    assert!(edge.label.is_none());
    let Some(Seg::Line(p)) = edge.segs.get(1) else {
        panic!("{:?}", edge.segs)
    };
    // The corner is 14 pixels below the card and rounds by half of that, the most the short
    // run allows.
    assert!(
        (p.y - (review.bottom() + 7.)).abs() < 0.01,
        "{p:?} {start:?}"
    );
}

#[test]
fn the_loop_pill_says_how_many_rounds_and_the_tooltip_who_decides() {
    let mono = LoopSpec {
        max_rounds: 3.,
        decide: "ai".into(),
    };
    assert_eq!(loop_label(&mono, false), "\u{2264} 3 rounds");
    assert_eq!(loop_label(&mono, true), "AI decides \u{b7} max 3");
    let once = LoopSpec {
        max_rounds: 1.,
        decide: "ai".into(),
    };
    assert_eq!(loop_label(&once, false), "\u{2264} 1 round");
    let short = LoopSpec {
        max_rounds: 2.,
        decide: "  {{r.v}} != \"ok\" ".into(),
    };
    assert_eq!(loop_label(&short, false), "\u{2264} 2 rounds");
    assert_eq!(
        loop_label(&short, true),
        "If {{r.v}} != \"ok\" \u{b7} max 2"
    );
    let long = LoopSpec {
        max_rounds: 10.,
        decide: "{{review.verdict}} != \"approve\" and more".into(),
    };
    assert_eq!(loop_label(&long, false), "\u{2264} 10 rounds");
    assert!(loop_label(&long, true).contains("and more"));
}

#[test]
fn a_for_each_takes_and_gives_its_lines_at_the_middle_of_its_frame() {
    let w = workflow(
        json!([
            {"id": "start", "type": "manual"},
            {"id": "each", "type": "forEach", "in": "{{trigger.items}}"},
            {"id": "dev", "type": "session", "parent": "each", "project": "{{inputs.project}}",
             "prompt": "x", "checkout": "root", "output": {}},
            {"id": "report", "type": "command", "project": "{{inputs.project}}", "command": "true"},
        ]),
        json!([{"from": "start", "to": "each"}, {"from": "each", "to": "report"}]),
    );
    let scene = scene_of(&w);
    let frame = node(&scene, "each").rect;
    let into = scene
        .edges
        .iter()
        .find(|e| e.id.contains("each") && e.id.contains("start"))
        .unwrap();
    assert_eq!(
        first_and_last(into).1,
        Point {
            x: frame.x,
            y: frame.y + frame.h / 2.
        }
    );
    let out = scene
        .edges
        .iter()
        .find(|e| e.id.starts_with("each"))
        .unwrap();
    let (start, _) = first_and_last(out);
    assert_eq!(
        start,
        Point {
            x: frame.right(),
            y: frame.y + frame.h / 2.
        }
    );
    // A card inside the frame is placed by the frame's corner, and the frames are drawn first.
    let dev = node(&scene, "dev");
    assert_eq!(dev.rect.x, frame.x + dev.flow.position.x);
    let order: Vec<&str> = scene.nodes.iter().map(|n| n.flow.id.as_str()).collect();
    assert!(order.iter().position(|id| *id == "each") < order.iter().position(|id| *id == "dev"));
}

#[test]
fn the_bounds_cover_the_cards_the_bars_and_the_loop_label() {
    let w = workflow(
        json!([{"id": "start", "type": "manual"}, session("dev"), session("review")]),
        json!([
            {"from": "start", "to": "dev"},
            {"from": "dev", "to": "review"},
            {"from": "review", "to": "dev", "loop": {"maxRounds": 3, "decide": "ai"}},
        ]),
    );
    let flow = to_flow(&w);
    let bar = Bar {
        id: "bar".into(),
        rect: Rect::new(-40., -60., 50., 20.),
        kind: BarKind::Lane {
            title: "x".into(),
            status: monocode_core::mono::workflows::run_graph_model::RunNodeStatus::Done,
        },
    };
    let scene = Scene::build(&flow.nodes, &flow.edges, vec![bar], "fit".into());
    let all = scene.bounds().unwrap();
    assert!(all.x <= -40. && all.y <= -60.);
    let label = scene.edges.iter().find_map(|e| e.label.as_ref()).unwrap();
    assert!(all.right() >= label.at.x + label.width);
    assert!(all.bottom() >= label.at.y + LABEL_H / 2.);
    assert!(
        Scene::build(&[], &[], Vec::new(), String::new())
            .bounds()
            .is_none()
    );
}

#[test]
fn run_states_travel_with_the_edges() {
    let w = workflow(
        json!([{"id": "start", "type": "manual"}, {"id": "report", "type": "command", "project": "{{inputs.project}}", "command": "true"}]),
        json!([{"from": "start", "to": "report"}]),
    );
    let flow = to_flow(&w);
    let mut edges = flow.edges.clone();
    edges[0].run = Some(RunEdgeState::Taken);
    let scene = Scene::build(&flow.nodes, &edges, Vec::new(), "fit".into());
    assert_eq!(scene.edges[0].state, Some(RunEdgeState::Taken));
}

#[test]
fn an_edge_to_a_node_that_is_not_drawn_is_left_out() {
    let w = workflow(
        json!([{"id": "start", "type": "manual"}, {"id": "report", "type": "command", "project": "{{inputs.project}}", "command": "true"}]),
        json!([{"from": "start", "to": "report"}]),
    );
    let flow = to_flow(&w);
    let only_trigger: Vec<_> = flow
        .nodes
        .iter()
        .filter(|n| n.id == "start")
        .cloned()
        .collect();
    let scene = Scene::build(&only_trigger, &flow.edges, Vec::new(), "fit".into());
    assert!(scene.edges.is_empty());
}

#[test]
fn the_loop_label_never_covers_a_card_the_loop_passes() {
    // A loop from the third card back to the first runs under the second.
    let w = workflow(
        json!([{"id": "start", "type": "manual"}, session("a"), session("b"), session("c")]),
        json!([
            {"from": "start", "to": "a"},
            {"from": "a", "to": "b"},
            {"from": "b", "to": "c"},
            {"from": "c", "to": "a", "loop": {"maxRounds": 3, "decide": "{{c.verdict}} != \"approve\" and a long reason"}},
        ]),
    );
    let scene = scene_of(&w);
    let label = scene.edges.iter().find_map(|e| e.label.as_ref()).unwrap();
    let lowest = ["a", "b", "c"]
        .iter()
        .map(|id| node(&scene, id).rect.bottom())
        .fold(f32::MIN, f32::max);
    assert!(
        label.at.y - LABEL_H / 2. > lowest,
        "{} vs {lowest}",
        label.at.y
    );
}
