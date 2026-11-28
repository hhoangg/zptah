//! The pure maths of the graph canvas: fit view, zoom, and the orthogonal edge paths. The paths
//! follow xyflow's smooth-step routing for the two cases the workflow graph uses (a line across
//! from the right of one card to the left of the next, and the loop edge under the bottoms).
use super::geometry::*;
use monocode_core::mono::workflows::board_model::{Point, Size};

fn at(x: f32, y: f32) -> Point {
    Point { x, y }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

fn same(a: Point, b: Point) -> bool {
    close(a.x, b.x) && close(a.y, b.y)
}

// --- viewport ---------------------------------------------------------------------------------

#[test]
fn fit_view_centres_content_that_is_smaller_than_the_view_at_zoom_one() {
    let v = fit_view(
        Rect::new(0., 0., 100., 100.),
        Size {
            width: 200.,
            height: 400.,
        },
        FIT_PADDING,
        FIT_MIN_ZOOM,
        FIT_MAX_ZOOM,
        None,
    );
    assert!(close(v.zoom, 1.));
    assert!(same(v.to_screen(at(50., 50.)), at(100., 200.)));
}

#[test]
fn fit_view_shrinks_big_content_to_the_view_with_the_padding() {
    let v = fit_view(
        Rect::new(100., 50., 1000., 500.),
        Size {
            width: 800.,
            height: 800.,
        },
        FIT_PADDING,
        FIT_MIN_ZOOM,
        FIT_MAX_ZOOM,
        None,
    );
    // The width is the tight side: 800 / (1000 * 1.12), above the floor.
    assert!(close(v.zoom, 800. / 1120.));
    assert!(same(v.to_screen(at(600., 300.)), at(400., 400.)));
}

#[test]
fn fit_view_never_goes_below_seventy_percent_or_above_its_own_maximum() {
    let size = Size {
        width: 400.,
        height: 400.,
    };
    let huge = fit_view(
        Rect::new(0., 0., 10_000., 10_000.),
        size,
        FIT_PADDING,
        FIT_MIN_ZOOM,
        FIT_MAX_ZOOM,
        None,
    );
    assert!(close(huge.zoom, 0.7));
    assert!(close(FIT_MIN_ZOOM, 0.7));
    let tiny = fit_view(
        Rect::new(0., 0., 10., 10.),
        size,
        FIT_PADDING,
        FIT_MIN_ZOOM,
        1.0,
        None,
    );
    assert!(close(tiny.zoom, 1.));
    let capped = fit_view(
        Rect::new(0., 0., 10., 10.),
        size,
        FIT_PADDING,
        FIT_MIN_ZOOM,
        1.5,
        None,
    );
    assert!(close(capped.zoom, 1.5));
}

#[test]
fn a_graph_too_big_for_the_floor_is_centred_on_the_picked_card() {
    let size = Size {
        width: 400.,
        height: 400.,
    };
    let content = Rect::new(0., 0., 5_000., 600.);
    let card = Rect::new(3_000., 200., 232., 48.);
    let v = fit_view(
        content,
        size,
        FIT_PADDING,
        FIT_MIN_ZOOM,
        FIT_MAX_ZOOM,
        Some(card),
    );
    assert!(close(v.zoom, 0.7));
    assert!(same(v.to_screen(at(3_116., 224.)), at(200., 200.)));
    // Without a picked card it is the middle of the graph.
    let plain = fit_view(content, size, FIT_PADDING, FIT_MIN_ZOOM, FIT_MAX_ZOOM, None);
    assert!(same(plain.to_screen(at(2_500., 300.)), at(200., 200.)));
}

#[test]
fn a_graph_that_fits_is_shown_whole_whatever_is_picked() {
    let size = Size {
        width: 1_000.,
        height: 600.,
    };
    let content = Rect::new(0., 0., 800., 200.);
    let card = Rect::new(600., 100., 232., 48.);
    let v = fit_view(
        content,
        size,
        FIT_PADDING,
        FIT_MIN_ZOOM,
        FIT_MAX_ZOOM,
        Some(card),
    );
    assert!(v.zoom >= 0.7);
    assert!(same(v.to_screen(at(400., 100.)), at(500., 300.)));
}

#[test]
fn a_canvas_without_room_is_not_fitted() {
    let content = Some(Rect::new(0., 0., 100., 100.));
    let none = Size {
        width: 0.,
        height: 300.,
    };
    assert_eq!(fit_scene(content, none, None), None);
    assert_eq!(
        fit_scene(
            content,
            Size {
                width: 300.,
                height: 0.
            },
            None
        ),
        None
    );
    let room = Size {
        width: 300.,
        height: 300.,
    };
    assert!(fit_scene(content, room, None).is_some());
    // Nothing to show still fits, to the origin.
    assert_eq!(fit_scene(None, room, None), Some(Viewport::default()));
}

#[test]
fn zoom_keeps_the_point_under_the_pointer_where_it_is() {
    let v = Viewport {
        x: 30.,
        y: -20.,
        zoom: 0.8,
    };
    let anchor = at(120., 90.);
    let before = v.to_graph(anchor);
    let z = zoom_about(v, 1.3, anchor);
    assert!(close(z.zoom, 1.3));
    assert!(same(z.to_screen(before), anchor));
}

#[test]
fn zoom_stays_between_the_limits() {
    let v = Viewport::default();
    assert!(close(zoom_about(v, 9., at(0., 0.)).zoom, MAX_ZOOM));
    assert!(close(zoom_about(v, 0.01, at(0., 0.)).zoom, MIN_ZOOM));
    assert!(close(clamp_zoom(f32::NAN), 1.));
}

#[test]
fn the_buttons_zoom_by_a_fifth_each_press() {
    assert!(close(zoom_in(1.), 1.2));
    assert!(close(zoom_out(1.2), 1.));
    assert!(close(zoom_in(1.4), MAX_ZOOM));
    assert!(close(zoom_out(0.31), MIN_ZOOM));
}

#[test]
fn bounds_cover_every_rect_and_nothing_gives_none() {
    assert_eq!(bounds_of([]), None);
    let all = bounds_of([
        Rect::new(10., 20., 30., 40.),
        Rect::new(-5., 50., 10., 100.),
    ])
    .unwrap();
    assert_eq!(all, Rect::new(-5., 20., 45., 130.));
}

// --- edge paths -------------------------------------------------------------------------------

#[test]
fn a_line_straight_across_has_no_bends() {
    let path = step_points(at(0., 100.), Side::Right, at(80., 100.), Side::Left, 20.);
    assert!(path.points.windows(2).all(|w| close(w[0].y, w[1].y)));
    let segs = rounded(&path.points, 5.);
    assert_eq!(segs.first(), Some(&Seg::Move(at(0., 100.))));
    assert_eq!(segs.last(), Some(&Seg::Line(at(80., 100.))));
    assert!(segs.iter().all(|s| !matches!(s, Seg::Quad { .. })));
}

#[test]
fn a_line_to_a_card_across_and_down_bends_half_way_across() {
    let path = step_points(at(0., 0.), Side::Right, at(100., 100.), Side::Left, 20.);
    let corners: Vec<Point> = path.points.clone();
    assert_eq!(
        corners,
        vec![at(0., 0.), at(50., 0.), at(50., 100.), at(100., 100.)]
    );
    assert!(same(path.label, at(50., 50.)));
    assert!(close(path.line, 50.));
    let segs = rounded(&path.points, 5.);
    assert_eq!(segs[0], Seg::Move(at(0., 0.)));
    // Each corner is a quadratic curve through the corner point, five pixels each way.
    assert_eq!(segs[1], Seg::Line(at(45., 0.)));
    assert_eq!(
        segs[2],
        Seg::Quad {
            ctrl: at(50., 0.),
            to: at(50., 5.)
        }
    );
    assert_eq!(segs[3], Seg::Line(at(50., 95.)));
    assert_eq!(
        segs[4],
        Seg::Quad {
            ctrl: at(50., 100.),
            to: at(55., 100.)
        }
    );
    assert_eq!(segs[5], Seg::Line(at(100., 100.)));
}

#[test]
fn a_short_hop_rounds_no_more_than_half_of_the_segment() {
    // 4 pixels between the bends: the curve takes two each side, not the full radius.
    let points = vec![at(0., 0.), at(4., 0.), at(4., 10.), at(8., 10.)];
    let segs = rounded(&points, 12.);
    assert_eq!(segs[1], Seg::Line(at(2., 0.)));
    assert_eq!(
        segs[2],
        Seg::Quad {
            ctrl: at(4., 0.),
            to: at(4., 2.)
        }
    );
}

#[test]
fn a_line_to_a_card_behind_goes_round_through_the_middle_row() {
    // A target left of its source: across, down at the source's gap, back along the middle row,
    // down, across.
    let path = step_points(at(100., 0.), Side::Right, at(0., 100.), Side::Left, 20.);
    assert_eq!(
        path.points,
        vec![
            at(100., 0.),
            at(120., 0.),
            at(120., 50.),
            at(-20., 50.),
            at(-20., 100.),
            at(0., 100.)
        ]
    );
}

#[test]
fn the_loop_edge_leaves_the_bottom_runs_back_under_the_cards_and_comes_in_from_below() {
    // Out of the later card's bottom, into the earlier card's bottom, 26 pixels down.
    let path = step_points(
        at(500., 48.),
        Side::Bottom,
        at(100., 48.),
        Side::Bottom,
        26.,
    );
    assert_eq!(
        path.points,
        vec![at(500., 48.), at(500., 74.), at(100., 74.), at(100., 48.)]
    );
    // The label belongs half way along the run under the cards.
    assert!(close(path.line, 74.));
    assert!(close(path.label.x, 300.));
}

#[test]
fn the_loop_edge_runs_under_the_lower_card_when_the_two_differ_in_height() {
    let path = step_points(
        at(500., 120.),
        Side::Bottom,
        at(100., 48.),
        Side::Bottom,
        14.,
    );
    assert!(close(path.line, 134.));
    assert_eq!(path.points[1], at(500., 134.));
    assert_eq!(path.points[2], at(100., 134.));
}

#[test]
fn the_loop_edge_rounds_its_two_corners_with_the_radius_it_is_given() {
    let path = step_points(at(300., 48.), Side::Bottom, at(0., 48.), Side::Bottom, 26.);
    let segs = rounded(&path.points, 12.);
    assert_eq!(segs[0], Seg::Move(at(300., 48.)));
    assert_eq!(segs[1], Seg::Line(at(300., 62.)));
    assert_eq!(
        segs[2],
        Seg::Quad {
            ctrl: at(300., 74.),
            to: at(288., 74.)
        }
    );
    assert_eq!(segs[3], Seg::Line(at(12., 74.)));
    assert_eq!(
        segs[4],
        Seg::Quad {
            ctrl: at(0., 74.),
            to: at(0., 62.)
        }
    );
    assert_eq!(segs[5], Seg::Line(at(0., 48.)));
}

#[test]
fn an_arrowhead_points_the_way_the_last_segment_goes() {
    let down = arrow_head(&[at(0., 0.), at(0., 50.)], 5., 3.5).unwrap();
    assert!(same(down[0], at(0., 50.)));
    assert!(same(down[1], at(-3.5, 45.)) || same(down[1], at(3.5, 45.)));
    assert!(close(down[1].y, 45.) && close(down[2].y, 45.));
    assert!(close(down[1].x, -down[2].x));
    let left = arrow_head(&[at(50., 10.), at(0., 10.)], 5., 3.5).unwrap();
    assert!(same(left[0], at(0., 10.)));
    assert!(close(left[1].x, 5.) && close(left[2].x, 5.));
    assert!(arrow_head(&[at(1., 1.)], 5., 3.5).is_none());
    assert!(arrow_head(&[at(1., 1.), at(1., 1.)], 5., 3.5).is_none());
}
