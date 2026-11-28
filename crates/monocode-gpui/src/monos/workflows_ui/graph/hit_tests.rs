//! The maths of editing on the canvas: where the handles of a card are, how far a drag moved a
//! node, which card a connection was dropped on and which edge a click was on.
use super::geometry::{Rect, Seg};
use super::hit::*;
use super::scene::{RoutedEdge, Scene, SceneNode};
use monocode_core::mono::workflows::{
    board_model::{GROUP_HEADER, NODE_H, NODE_W, Point, to_flow},
    model::Workflow,
    ordered::Json,
    parse::parse_workflow,
};
use serde_json::{Value, json};

fn workflow(nodes: Value, edges: Value) -> Workflow {
    parse_workflow(&Json::from(&json!({
        "id": "wf-aaaaaaaa", "name": "Nightly checks", "inputs": {}, "inputValues": {},
        "nodes": nodes, "edges": edges, "enabled": true, "createdAt": 1,
        "ui": {"direction": "lr"},
    })))
    .expect("a workflow")
}

fn session(id: &str, x: f64, y: f64) -> Value {
    json!({"id": id, "type": "session", "project": "{{inputs.project}}", "prompt": "Do it",
           "checkout": "root", "output": {}, "ui": {"x": x, "y": y}})
}

fn scene() -> Scene {
    let w = workflow(
        json!([
            {"id": "start", "type": "manual"},
            session("a", 336., 100.),
            {"id": "check", "type": "if", "decide": "ai", "ui": {"x": 736, "y": 100}},
            {"id": "each", "type": "forEach", "in": "{{a.list}}", "ui": {"x": 336, "y": 400}},
            {"id": "report", "type": "command", "project": "{{inputs.project}}", "command": "true", "ui": {"x": 1100, "y": 100}},
            {"id": "route", "type": "switch", "value": "{{a.kind}}",
             "cases": ["bug", "feature", "chore"], "ui": {"x": 736, "y": 400}},
        ]),
        json!([
            {"from": "start", "to": "a"}, {"from": "a", "to": "check"},
            {"from": "route", "to": "report", "handle": "feature"},
            {"from": "route", "to": "a", "handle": "default"},
        ]),
    );
    let flow = to_flow(&w);
    Scene::build(&flow.nodes, &flow.edges, Vec::new(), "k".into())
}

fn node<'a>(scene: &'a Scene, id: &str) -> &'a SceneNode {
    scene.nodes.iter().find(|n| n.flow.id == id).expect(id)
}

fn kinds(scene: &Scene, id: &str) -> Vec<HandleKind> {
    handles_of(node(scene, id))
        .into_iter()
        .map(|h| h.kind)
        .collect()
}

#[test]
fn a_trigger_has_only_an_out_handle() {
    assert_eq!(kinds(&scene(), "start"), vec![HandleKind::Out(None)]);
}

#[test]
fn a_session_has_in_out_and_both_loop_handles() {
    let kinds = kinds(&scene(), "a");
    assert!(kinds.contains(&HandleKind::In));
    assert!(kinds.contains(&HandleKind::Out(None)));
    assert!(kinds.contains(&HandleKind::LoopIn));
    assert!(kinds.contains(&HandleKind::LoopOut));
}

#[test]
fn an_if_leaves_by_true_and_false_and_cannot_start_a_loop() {
    let kinds = kinds(&scene(), "check");
    assert!(kinds.contains(&HandleKind::Out(Some("true".into()))));
    assert!(kinds.contains(&HandleKind::Out(Some("false".into()))));
    assert!(!kinds.contains(&HandleKind::Out(None)));
    assert!(!kinds.contains(&HandleKind::LoopOut));
    assert!(kinds.contains(&HandleKind::LoopIn));
}

#[test]
fn a_switch_leaves_by_each_case_and_default_spread_down_its_right_side() {
    let scene = scene();
    let outs: Vec<(Option<String>, f32)> = handles_of(node(&scene, "route"))
        .into_iter()
        .filter_map(|h| match h.kind {
            HandleKind::Out(name) => Some((name, h.at.y - node(&scene, "route").rect.y)),
            _ => None,
        })
        .collect();
    let names: Vec<_> = outs.iter().map(|(n, _)| n.as_deref().unwrap()).collect();
    assert_eq!(names, vec!["bug", "feature", "chore", "default"]);
    // In order from top to bottom, apart, and all inside the card's height, on its right side.
    for pair in outs.windows(2) {
        assert!(pair[1].1 - pair[0].1 > 10., "{outs:?}");
    }
    assert!(
        outs[0].1 > 0. && outs[3].1 < node(&scene, "route").rect.h,
        "{outs:?}"
    );
    let route = node(&scene, "route").rect;
    assert!(
        handles_of(node(&scene, "route"))
            .iter()
            .filter(|h| matches!(h.kind, HandleKind::Out(_)))
            .all(|h| h.at.x == route.right())
    );
    let kinds = kinds(&scene, "route");
    assert!(!kinds.contains(&HandleKind::Out(None)));
    assert!(!kinds.contains(&HandleKind::LoopOut));
    assert!(kinds.contains(&HandleKind::LoopIn));
}

#[test]
fn the_line_of_a_case_leaves_from_the_handle_of_that_case() {
    let scene = scene();
    for (target, case) in [("report", "feature"), ("a", "default")] {
        let handle = handles_of(node(&scene, "route"))
            .into_iter()
            .find(|h| h.kind == HandleKind::Out(Some(case.into())))
            .unwrap();
        let edge = scene
            .edges
            .iter()
            .find(|e| e.id == format!("route.{case}->{target}"))
            .unwrap();
        assert_eq!(edge.segs[0], Seg::Move(handle.at), "{case}");
    }
}

#[test]
fn a_command_takes_and_gives_lines_and_loops_like_a_session() {
    let kinds = kinds(&scene(), "report");
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert!(kinds.contains(&HandleKind::In));
    assert!(kinds.contains(&HandleKind::Out(None)));
    assert!(kinds.contains(&HandleKind::LoopIn));
    assert!(kinds.contains(&HandleKind::LoopOut));
}

#[test]
fn a_for_each_takes_on_its_left_and_gives_on_its_right_at_the_middle() {
    let scene = scene();
    let group = node(&scene, "each");
    let handles = handles_of(group);
    let y = group.rect.y + group.rect.h / 2.;
    let left = handles.iter().find(|h| h.kind == HandleKind::In).unwrap();
    let right = handles
        .iter()
        .find(|h| h.kind == HandleKind::Out(None))
        .unwrap();
    assert_eq!(left.at, Point { x: group.rect.x, y });
    assert_eq!(
        right.at,
        Point {
            x: group.rect.right(),
            y
        }
    );
    assert!(!handles.iter().any(|h| h.kind == HandleKind::LoopOut));
}

#[test]
fn the_handles_sit_where_the_edges_leave_and_arrive() {
    let scene = scene();
    let a = node(&scene, "a");
    let at = |kind| {
        handles_of(a)
            .iter()
            .find(|h| h.kind == kind)
            .map(|h| h.at)
            .unwrap()
    };
    assert_eq!(
        at(HandleKind::In),
        Point {
            x: a.rect.x,
            y: a.rect.y + NODE_H / 2.
        }
    );
    assert_eq!(
        at(HandleKind::Out(None)),
        Point {
            x: a.rect.right(),
            y: a.rect.y + NODE_H / 2.
        }
    );
    assert_eq!(
        at(HandleKind::LoopOut),
        Point {
            x: a.rect.x + NODE_W * 0.7,
            y: a.rect.bottom()
        }
    );
    assert_eq!(
        at(HandleKind::LoopIn),
        Point {
            x: a.rect.x + NODE_W * 0.3,
            y: a.rect.bottom()
        }
    );
    // The edge from a to the If starts exactly on the out handle.
    let edge = &scene
        .edges
        .iter()
        .find(|e| e.id.starts_with("a->"))
        .unwrap();
    assert_eq!(edge.segs[0], Seg::Move(at(HandleKind::Out(None))));
}

#[test]
fn a_drag_moves_a_node_by_the_pointer_travel_in_graph_pixels() {
    let origin = Point { x: 100., y: 50. };
    let start = Point { x: 300., y: 200. };
    let now = Point { x: 360., y: 170. };
    assert_eq!(
        dragged_to(origin, start, now, 1.),
        Point { x: 160., y: 20. }
    );
    assert_eq!(
        dragged_to(origin, start, now, 0.5),
        Point { x: 220., y: -10. }
    );
}

#[test]
fn a_drag_starts_past_two_pixels() {
    let start = Point { x: 10., y: 10. };
    assert!(!past_threshold(start, Point { x: 11., y: 11. }));
    assert!(!past_threshold(start, Point { x: 12., y: 10. }));
    assert!(past_threshold(start, Point { x: 12.1, y: 10. }));
    assert!(past_threshold(start, Point { x: 10., y: 14. }));
}

#[test]
fn a_connection_dropped_on_a_card_connects_to_it() {
    let scene = scene();
    let a = node(&scene, "a").rect;
    let inside = Point {
        x: a.x + 20.,
        y: a.y + 20.,
    };
    assert_eq!(
        drop_target(inside, &scene.nodes, "start", 1.).as_deref(),
        Some("a")
    );
    // Not to the card it started from.
    assert_eq!(drop_target(inside, &scene.nodes, "a", 1.), None);
}

#[test]
fn a_connection_dropped_a_little_outside_a_card_still_finds_it() {
    let scene = scene();
    let a = node(&scene, "a").rect;
    let near = Point {
        x: a.x - 10.,
        y: a.y - 10.,
    };
    assert_eq!(
        drop_target(near, &scene.nodes, "start", 1.).as_deref(),
        Some("a")
    );
    let far = Point {
        x: a.x - 60.,
        y: a.y - 60.,
    };
    assert_eq!(drop_target(far, &scene.nodes, "start", 1.), None);
}

#[test]
fn the_drop_radius_is_in_screen_pixels() {
    let scene = scene();
    let a = node(&scene, "a").rect;
    let near = Point {
        x: a.x - 15.,
        y: a.y + 10.,
    };
    // 15 graph pixels is 30 on screen at zoom 2: too far; at zoom 0.5 it is 7.5: close enough.
    assert_eq!(drop_target(near, &scene.nodes, "start", 2.), None);
    assert_eq!(
        drop_target(near, &scene.nodes, "start", 0.5).as_deref(),
        Some("a")
    );
}

#[test]
fn a_for_each_is_a_target_by_its_header_not_by_its_empty_inside() {
    let scene = scene();
    let g = node(&scene, "each").rect;
    let header = Point {
        x: g.x + 40.,
        y: g.y + GROUP_HEADER / 2.,
    };
    assert_eq!(
        drop_target(header, &scene.nodes, "start", 1.).as_deref(),
        Some("each")
    );
    let inside = Point {
        x: g.x + 40.,
        y: g.y + g.h - 10.,
    };
    assert_eq!(drop_target(inside, &scene.nodes, "start", 1.), None);
}

#[test]
fn a_card_wins_over_the_frame_it_sits_in() {
    let w = workflow(
        json!([
            {"id": "start", "type": "manual"},
            {"id": "each", "type": "forEach", "in": "x", "ui": {"x": 400, "y": 0}},
            {"id": "dev", "type": "command", "project": "p", "command": "ls", "parent": "each",
             "ui": {"x": 16, "y": 40}},
        ]),
        json!([]),
    );
    let flow = to_flow(&w);
    let scene = Scene::build(&flow.nodes, &flow.edges, Vec::new(), "k".into());
    let dev = node(&scene, "dev").rect;
    let over = Point {
        x: dev.x + 5.,
        y: dev.y + 5.,
    };
    assert_eq!(
        drop_target(over, &scene.nodes, "start", 1.).as_deref(),
        Some("dev")
    );
}

fn straight(id: &str, a: Point, b: Point) -> RoutedEdge {
    RoutedEdge {
        id: id.into(),
        is_loop: false,
        state: None,
        segs: vec![Seg::Move(a), Seg::Line(b)],
        arrow: None,
        label: None,
        selected: false,
    }
}

#[test]
fn a_click_near_a_line_picks_the_edge_and_far_from_it_picks_nothing() {
    let edges = vec![
        straight("x", Point { x: 0., y: 0. }, Point { x: 100., y: 0. }),
        straight("y", Point { x: 0., y: 50. }, Point { x: 0., y: 150. }),
    ];
    assert_eq!(
        edge_at(Point { x: 50., y: 4. }, &edges).as_deref(),
        Some("x")
    );
    assert_eq!(
        edge_at(Point { x: 3., y: 100. }, &edges).as_deref(),
        Some("y")
    );
    assert_eq!(edge_at(Point { x: 50., y: 30. }, &edges), None);
    // Beyond the end of a line is not on it.
    assert_eq!(edge_at(Point { x: 130., y: 0. }, &edges), None);
}

#[test]
fn a_rounded_corner_is_on_the_edge_too() {
    let corner = RoutedEdge {
        segs: vec![
            Seg::Move(Point { x: 0., y: 0. }),
            Seg::Line(Point { x: 50., y: 0. }),
            Seg::Quad {
                ctrl: Point { x: 60., y: 0. },
                to: Point { x: 60., y: 10. },
            },
            Seg::Line(Point { x: 60., y: 80. }),
        ],
        ..straight("c", Point::default(), Point::default())
    };
    // The middle of the curve is nearer the corner point than the chord.
    assert_eq!(
        edge_at(Point { x: 57.5, y: 2.5 }, std::slice::from_ref(&corner)).as_deref(),
        Some("c")
    );
    assert_eq!(
        edge_at(Point { x: 60., y: 40. }, &[corner]).as_deref(),
        Some("c")
    );
}

#[test]
fn the_closest_edge_wins() {
    let edges = vec![
        straight("far", Point { x: 0., y: 5. }, Point { x: 100., y: 5. }),
        straight("near", Point { x: 0., y: 1. }, Point { x: 100., y: 1. }),
    ];
    assert_eq!(
        edge_at(Point { x: 50., y: 0. }, &edges).as_deref(),
        Some("near")
    );
}

#[test]
fn a_rect_is_inflated_on_all_sides() {
    let r = Rect::new(10., 10., 20., 20.);
    assert_eq!(inflate(r, 5.), Rect::new(5., 5., 30., 30.));
}
