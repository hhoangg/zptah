use super::*;
use monocode_core::diagram::{FlowEdge, FlowNode};

fn scene() -> Arc<FlowScene> {
    let node = |id: &str, label: &str, kind| FlowNode {
        id: id.into(),
        label: label.into(),
        kind,
        color: None,
    };
    let edge = |from: &str, to: &str, label: Option<&str>| FlowEdge {
        from: from.into(),
        to: to.into(),
        label: label.map(str::to_owned),
        dashed: false,
        arrow: true,
    };
    FlowScene::new(FlowGraph {
        direction: diagram::Direction::LR,
        nodes: vec![
            node("a", "Request", NodeKind::Start),
            node("b", "Risk?", NodeKind::Decision),
            node("c", "Auto act", NodeKind::Box),
        ],
        edges: vec![edge("a", "b", None), edge("b", "c", Some("safe"))],
    })
    .unwrap()
}

type Probe = Rc<RefCell<Option<Entity<ViewerState>>>>;

struct Fixture {
    scene: Arc<FlowScene>,
    probe: Probe,
}

impl gpui::Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(700.)).child(
            DiagramViewer::new(
                "t",
                "flow",
                "Test flow",
                ViewerContent::Flow(self.scene.clone()),
            )
            .source(
                "source".into(),
                Box::new(|| div().size_full().child("source text").into_any_element()),
            )
            .probe(self.probe.clone()),
        )
    }
}

fn open(cx: &mut gpui::TestAppContext) -> (Probe, &'static mut gpui::VisualTestContext) {
    let probe = Probe::default();
    let handed = probe.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|_| Fixture {
                scene: scene(),
                probe: handed,
            })
        })
        .unwrap()
    });
    let visual = gpui::VisualTestContext::from_window(window.into(), cx).into_mut();
    visual.run_until_parked();
    // Paint fits the first view and defers a refresh for the zoom label.
    visual.run_until_parked();
    (probe, visual)
}

fn state(probe: &Probe) -> Entity<ViewerState> {
    probe.borrow().clone().unwrap()
}

#[gpui::test]
fn copying_shows_check_then_resets_and_repeated_copy_restarts_feedback(
    cx: &mut gpui::TestAppContext,
) {
    let viewer = cx.new(ViewerState::new);
    viewer.update(cx, |state, cx| state.copy("graph LR; A-->B\r\n", cx));
    assert!(viewer.read_with(cx, |state, _| state.copied));
    assert_eq!(
        cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
            .as_deref(),
        Some("graph LR; A-->B")
    );
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(1000));
    viewer.update(cx, |state, cx| state.copy("graph LR; A-->C\n", cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(1000));
    cx.run_until_parked();
    assert!(viewer.read_with(cx, |state, _| state.copied));
    cx.executor().advance_clock(Duration::from_millis(500));
    cx.run_until_parked();
    assert!(!viewer.read_with(cx, |state, _| state.copied));
}

#[gpui::test]
fn first_paint_fits_and_the_toolbar_zooms_about_the_board_centre(cx: &mut gpui::TestAppContext) {
    let (probe, visual) = open(cx);
    let viewer = state(&probe);
    let fitted = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    let board = viewer.read_with(visual, |s, _| s.shared.borrow().board);
    assert!(board[0] > 300. && board[1] >= MIN_BOARD - 1.);
    assert!(fitted.scale > 0.25 && fitted.scale <= 1.6);
    // Fit centres the content.
    let content = viewer.read_with(visual, |s, _| s.shared.borrow().content);
    assert!((fitted.x - (board[0] - content[0] * fitted.scale) / 2.).abs() < 0.5);
    let level = visual.debug_bounds("flow-zoom-level").unwrap();
    assert!(level.size.width > px(0.));

    let centre = [board[0] / 2., board[1] / 2.];
    let before = fitted.to_content(centre[0], centre[1]);
    let zoom_in = visual.debug_bounds("flow-zoom-in").unwrap();
    visual.simulate_click(zoom_in.center(), Default::default());
    visual.run_until_parked();
    let zoomed = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    assert!((zoomed.scale / fitted.scale - 1.2).abs() < 1e-3);
    let after = zoomed.to_content(centre[0], centre[1]);
    assert!((before[0] - after[0]).abs() < 0.01 && (before[1] - after[1]).abs() < 0.01);
    let zoom_out = visual.debug_bounds("flow-zoom-out").unwrap();
    visual.simulate_click(zoom_out.center(), Default::default());
    visual.run_until_parked();
    let back = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    assert!((back.scale - fitted.scale).abs() < 1e-3);
    visual.simulate_click(zoom_in.center(), Default::default());
    visual.run_until_parked();
    let fit = visual.debug_bounds("flow-fit").unwrap();
    visual.simulate_click(fit.center(), Default::default());
    visual.run_until_parked();
    let refit = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    assert!((refit.scale - fitted.scale).abs() < 1e-3 && (refit.x - fitted.x).abs() < 0.5);
}

#[gpui::test]
fn dragging_pans_and_double_click_fits(cx: &mut gpui::TestAppContext) {
    let (probe, visual) = open(cx);
    let viewer = state(&probe);
    let start = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    let board = visual.debug_bounds("flow-image").unwrap();
    let from = board.center();
    visual.simulate_mouse_down(from, MouseButton::Left, Default::default());
    visual.simulate_mouse_move(
        from + point(px(40.), px(25.)),
        MouseButton::Left,
        Default::default(),
    );
    visual.simulate_mouse_up(
        from + point(px(40.), px(25.)),
        MouseButton::Left,
        Default::default(),
    );
    visual.run_until_parked();
    let moved = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    assert!((moved.x - start.x - 40.).abs() < 0.5 && (moved.y - start.y - 25.).abs() < 0.5);
    assert_eq!(moved.scale, start.scale);
    // A move after release no longer pans.
    visual.simulate_mouse_move(from + point(px(90.), px(90.)), None, Default::default());
    let still = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    assert_eq!(still, moved);
    visual.simulate_click(from, Default::default());
    visual.simulate_click(from, Default::default());
    visual.run_until_parked();
}

#[gpui::test]
fn wheel_zooms_at_the_pointer(cx: &mut gpui::TestAppContext) {
    let (probe, visual) = open(cx);
    let viewer = state(&probe);
    let board = visual.debug_bounds("flow-image").unwrap();
    let at = board.origin + point(px(120.), px(60.));
    let before = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    let content_point = before.to_content(120., 60.);
    visual.simulate_event(gpui::ScrollWheelEvent {
        position: at,
        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(-40.))),
        modifiers: Default::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    visual.run_until_parked();
    let after = viewer.read_with(visual, |s, _| s.shared.borrow().view);
    assert!(after.scale > before.scale);
    let held = after.to_content(120., 60.);
    assert!((held[0] - content_point[0]).abs() < 0.01 && (held[1] - content_point[1]).abs() < 0.01);
}

#[gpui::test]
fn fullscreen_fills_the_window_and_escape_exits(cx: &mut gpui::TestAppContext) {
    let (probe, visual) = open(cx);
    let viewer = state(&probe);
    let small = visual.debug_bounds("flow-panel").unwrap();
    let full = visual.debug_bounds("flow-fullscreen").unwrap();
    visual.simulate_click(full.center(), Default::default());
    visual.run_until_parked();
    assert!(viewer.read_with(visual, |s, _| s.fullscreen));
    let large = visual.debug_bounds("flow-panel").unwrap();
    assert!(large.size.height > small.size.height * 1.5);
    visual.simulate_keystrokes("escape");
    visual.run_until_parked();
    assert!(!viewer.read_with(visual, |s, _| s.fullscreen));
    assert_eq!(
        visual.debug_bounds("flow-panel").unwrap().size.height,
        small.size.height
    );
}

#[gpui::test]
fn source_toggle_swaps_the_board_without_changing_height(cx: &mut gpui::TestAppContext) {
    let (_, visual) = open(cx);
    let board = visual.debug_bounds("flow-image").unwrap();
    assert!(visual.debug_bounds("flow-source").is_none());
    let toggle = visual.debug_bounds("flow-toggle").unwrap();
    visual.simulate_click(toggle.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(
        visual.debug_bounds("flow-source").unwrap().size.height,
        board.size.height
    );
    assert!(visual.debug_bounds("flow-image").is_none());
}

#[test]
fn board_height_follows_content_within_limits() {
    assert_eq!(board_height([100., 10.], None), MIN_BOARD);
    assert_eq!(board_height([100., 9000.], None), MAX_BOARD);
    assert_eq!(board_height([100., 200.], None), 240.);
    assert_eq!(board_height([100., 200.], Some(300.)), 300.);
    assert_eq!(board_height([100., 200.], Some(900.)), MAX_BOARD);
}
