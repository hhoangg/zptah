//! The one diagram viewer: Mermaid blocks and `visual.flow` both render here.
//! A dotted board that pans and zooms with its content, a toolbar (zoom out,
//! zoom %, zoom in, Fit, Fullscreen, Source, Copy), drag to pan, wheel and pinch
//! zoom at the pointer, double click to fit, fit on first paint.
//!
//! Content is either a vector `FlowScene` (laid out by `monocode_core::diagram`)
//! or a raster image (Mermaid diagram kinds the flow parser does not cover).
//! All pan and zoom math lives in `monocode_core::diagram::viewport`.
use crate::theme::{content, floating_fill};
use crate::visuals::{color, paint_path};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Edges, ElementId,
    Entity, FocusHandle, Hsla, IntoElement, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, RenderImage, RenderOnce, Role, SharedString, Task, Window, canvas, div, point,
    prelude::*, px, quad, size,
};
use monocode_core::diagram::{
    self, FlowGraph, Layout, NodeKind,
    layout::{EDGE_FONT, LINE_HEIGHT, NODE_FONT, arrowhead, dash_polyline, smooth},
    viewport::{DOT_SPACING, Viewport, pinch_factor, wheel_factor},
};
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

const BUTTON_STEP: f32 = diagram::viewport::BUTTON_STEP;
const MIN_BOARD: f32 = 180.;
pub(crate) const MAX_BOARD: f32 = 420.;
const EDGE_RADIUS: f32 = 8.;
const ARROW: f32 = 9.;

/// A laid-out flow diagram ready to paint.
pub(crate) struct FlowScene {
    pub graph: FlowGraph,
    pub layout: Layout,
}

impl FlowScene {
    pub(crate) fn new(graph: FlowGraph) -> Result<Arc<Self>, String> {
        let layout = diagram::layout(&graph)?;
        Ok(Arc::new(Self { graph, layout }))
    }
}

#[derive(Clone)]
pub(crate) enum ViewerContent {
    Flow(Arc<FlowScene>),
    Image(Arc<RenderImage>),
    Loading,
    /// Nothing to draw (an error): the caller shows its message and the source.
    Empty,
}

impl ViewerContent {
    fn size(&self) -> [f32; 2] {
        match self {
            Self::Flow(scene) => [scene.layout.width, scene.layout.height],
            // Rasterised at up to 2x.
            Self::Image(image) => {
                let size = image.size(0);
                [
                    (size.width.0 as f32 / 2.).max(1.),
                    (size.height.0 as f32 / 2.).max(1.),
                ]
            }
            Self::Loading | Self::Empty => [1., 1.],
        }
    }
}

/// The height of the board outside fullscreen for content of this size.
pub(crate) fn board_height(content: [f32; 2], override_height: Option<f32>) -> f32 {
    override_height
        .unwrap_or(content[1] + 2. * diagram::viewport::FIT_MARGIN)
        .clamp(MIN_BOARD, MAX_BOARD)
}

#[derive(Default)]
struct Shared {
    view: Viewport,
    origin: [f32; 2],
    board: [f32; 2],
    content: [f32; 2],
    fitted: bool,
    /// The user panned or zoomed: a resize keeps their view instead of refitting.
    touched: bool,
    drag: Option<([f32; 2], [f32; 2])>,
}

impl Shared {
    fn fit(&mut self) {
        self.view = Viewport::fit(self.content, self.board);
        self.fitted = true;
        self.touched = false;
    }
    fn local(&self, p: Point<Pixels>) -> [f32; 2] {
        [p.x.as_f32() - self.origin[0], p.y.as_f32() - self.origin[1]]
    }
}

pub(crate) struct ViewerState {
    shared: Rc<RefCell<Shared>>,
    pub(crate) source: bool,
    pub(crate) copied: bool,
    pub(crate) fullscreen: bool,
    copy_reset: Option<Task<()>>,
    focus: FocusHandle,
}

impl ViewerState {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            shared: Rc::default(),
            source: false,
            copied: false,
            fullscreen: false,
            copy_reset: None,
            focus: cx.focus_handle(),
        }
    }

    pub(crate) fn percent(&self) -> u32 {
        self.shared.borrow().view.percent()
    }

    fn zoom_centre(&mut self, factor: f32, cx: &mut Context<Self>) {
        {
            let mut shared = self.shared.borrow_mut();
            let (cx_, cy) = (shared.board[0] / 2., shared.board[1] / 2.);
            shared.view.zoom_at(factor, cx_, cy);
            shared.touched = true;
        }
        cx.notify();
    }

    fn fit(&mut self, cx: &mut Context<Self>) {
        self.shared.borrow_mut().fit();
        cx.notify();
    }

    fn set_fullscreen(&mut self, on: bool, cx: &mut Context<Self>) {
        self.fullscreen = on;
        {
            let mut shared = self.shared.borrow_mut();
            shared.fitted = false;
            shared.touched = false;
        }
        cx.notify();
    }

    pub(crate) fn copy(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(
            text.strip_suffix("\r\n")
                .or_else(|| text.strip_suffix('\n'))
                .unwrap_or(text)
                .to_owned(),
        ));
        self.copied = true;
        cx.notify();
        self.copy_reset = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.copied = false;
                this.copy_reset = None;
                cx.notify();
            });
        }));
    }
}

/// Lazily builds the source view; the viewer calls it only while shown.
pub(crate) type SourceView = Box<dyn FnOnce() -> AnyElement>;

#[derive(IntoElement)]
pub(crate) struct DiagramViewer {
    id: SharedString,
    prefix: &'static str,
    title: SharedString,
    icon: Option<&'static str>,
    content: ViewerContent,
    aria: SharedString,
    height: Option<f32>,
    copy_text: Option<SharedString>,
    source_view: Option<SourceView>,
    force_source: bool,
    status: Option<String>,
    source_status: Option<String>,
    #[cfg(test)]
    probe: Option<Rc<RefCell<Option<Entity<ViewerState>>>>>,
}

impl DiagramViewer {
    pub(crate) fn new(
        id: impl Into<SharedString>,
        prefix: &'static str,
        title: impl Into<SharedString>,
        content: ViewerContent,
    ) -> Self {
        Self {
            id: id.into(),
            prefix,
            title: title.into(),
            icon: None,
            content,
            aria: "Diagram".into(),
            height: None,
            copy_text: None,
            source_view: None,
            force_source: false,
            status: None,
            source_status: None,
            #[cfg(test)]
            probe: None,
        }
    }
    pub(crate) fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }
    pub(crate) fn aria(mut self, aria: impl Into<SharedString>) -> Self {
        self.aria = aria.into();
        self
    }
    pub(crate) fn height(mut self, height: Option<f32>) -> Self {
        self.height = height;
        self
    }
    /// Enables the Source toggle and the Copy button.
    pub(crate) fn source(mut self, text: SharedString, view: SourceView) -> Self {
        self.copy_text = Some(text);
        self.source_view = Some(view);
        self
    }
    /// Show the source instead of the board (an error, nothing to draw).
    pub(crate) fn force_source(mut self, force: bool) -> Self {
        self.force_source = force;
        self
    }
    /// A note shown only while the source is displayed.
    pub(crate) fn source_status(mut self, status: impl Into<String>) -> Self {
        self.source_status = Some(status.into());
        self
    }
    #[cfg(test)]
    fn probe(mut self, probe: Rc<RefCell<Option<Entity<ViewerState>>>>) -> Self {
        self.probe = Some(probe);
        self
    }
    pub(crate) fn status(mut self, status: Option<String>) -> Self {
        self.status = status;
        self
    }
}

fn tool_button(
    id: SharedString,
    text: impl Into<SharedString>,
    aria: &'static str,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(ElementId::from(id))
        .role(Role::Button)
        .aria_label(aria)
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .h(px(28.))
        .min_w(px(28.))
        .px(px(8.))
        .rounded(px(8.))
        .text_size(px(13.))
        .text_color(content(0.6))
        .cursor_pointer()
        .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
        .child(text.into())
}

impl RenderOnce for DiagramViewer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("{}-viewer-state", self.id)),
            cx,
            |_, cx| ViewerState::new(cx),
        );
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            *probe.borrow_mut() = Some(state.clone());
        }
        let content_size = self.content.size();
        let (source_open, copied, fullscreen, percent, shared, focus) = {
            let s = state.read(cx);
            {
                let mut shared = s.shared.borrow_mut();
                if shared.content != content_size {
                    shared.content = content_size;
                    shared.fitted = false;
                }
            }
            (
                s.source,
                s.copied,
                s.fullscreen,
                s.percent(),
                s.shared.clone(),
                s.focus.clone(),
            )
        };
        let prefix = self.prefix;
        let has_source = self.source_view.is_some();
        let show_source = has_source && source_open || self.force_source;
        let board_h = board_height(content_size, self.height);
        let sel = move |name: &'static str| move || format!("{prefix}-{name}");

        let mut toolbar = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(10.))
            .min_h(px(40.))
            .text_size(px(12.))
            .text_color(content(0.65))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .when_some(self.icon, |d, icon| {
                        d.child(crate::activity::file_icon(icon, 16.))
                    })
                    .child(div().min_w_0().truncate().child(self.title.clone())),
            );
        let id = &self.id;
        let button_id = |name: &str| SharedString::from(format!("{id}-{name}"));
        let zoom_out = {
            let state = state.clone();
            tool_button(button_id("out"), "\u{2212}", "Zoom out")
                .debug_selector(sel("zoom-out"))
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.zoom_centre(1. / BUTTON_STEP, cx));
                    cx.stop_propagation();
                })
        };
        let zoom_in = {
            let state = state.clone();
            tool_button(button_id("in"), "+", "Zoom in")
                .debug_selector(sel("zoom-in"))
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.zoom_centre(BUTTON_STEP, cx));
                    cx.stop_propagation();
                })
        };
        let fit = {
            let state = state.clone();
            tool_button(button_id("fit"), "Fit", "Fit to view")
                .debug_selector(sel("fit"))
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.fit(cx));
                    cx.stop_propagation();
                })
        };
        let full = {
            let state = state.clone();
            let focus = focus.clone();
            tool_button(
                button_id("full"),
                if fullscreen { "Exit" } else { "Full" },
                if fullscreen {
                    "Exit fullscreen"
                } else {
                    "Fullscreen"
                },
            )
            .debug_selector(sel("fullscreen"))
            .on_click(move |_, window, cx| {
                let on = !state.read(cx).fullscreen;
                state.update(cx, |s, cx| s.set_fullscreen(on, cx));
                if on {
                    window.focus(&focus, cx);
                }
                cx.stop_propagation();
            })
        };
        toolbar = toolbar
            .child(zoom_out)
            .child(
                div()
                    .debug_selector(sel("zoom-level"))
                    .min_w(px(46.))
                    .text_center()
                    .text_size(px(12.))
                    .text_color(content(0.6))
                    .child(format!("{percent}%")),
            )
            .child(zoom_in)
            .child(fit)
            .child(full);
        if has_source {
            let toggle_state = state.clone();
            toolbar = toolbar.child(
                tool_button(
                    button_id("toggle"),
                    if show_source { "Diagram" } else { "Source" },
                    if show_source {
                        "Show diagram"
                    } else {
                        "Show source"
                    },
                )
                .debug_selector(sel("toggle"))
                .on_click(move |_, _, cx| {
                    toggle_state.update(cx, |s, cx| {
                        s.source = !s.source;
                        cx.notify();
                    });
                    cx.stop_propagation();
                }),
            );
            let copy_state = state.clone();
            let text = self.copy_text.clone().unwrap_or_default();
            toolbar = toolbar.child(
                tool_button(
                    button_id("copy"),
                    if copied { "Copied" } else { "Copy" },
                    if copied { "Copied" } else { "Copy code" },
                )
                .debug_selector(sel("copy"))
                .on_click(move |_, _, cx| {
                    copy_state.update(cx, |s, cx| s.copy(&text, cx));
                    cx.stop_propagation();
                }),
            );
        }

        let body_height = move |d: gpui::Div| {
            if fullscreen {
                d.flex_1().min_h_0()
            } else {
                d.h(px(board_h))
            }
        };
        let media = if show_source {
            let view = self.source_view.map(|build| build());
            body_height(div().w_full().debug_selector(sel("source")))
                .when_some(view, |d, view| d.child(view))
                .into_any_element()
        } else {
            let content = self.content.clone();
            let loading = matches!(content, ViewerContent::Loading);
            let paint_state = state.clone();
            let paint_shared = shared.clone();
            let paint_focus = focus.clone();
            let board = canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    paint_board(
                        bounds,
                        &paint_shared,
                        &paint_state,
                        &content,
                        &paint_focus,
                        window,
                        cx,
                    )
                },
            )
            .absolute()
            .size_full();
            let down_shared = shared.clone();
            let down_state = state.clone();
            let wheel_shared = shared.clone();
            let wheel_state = state.clone();
            let pinch_shared = shared.clone();
            let pinch_state = state.clone();
            body_height(div().w_full().relative().overflow_hidden())
                .id(ElementId::from(button_id("board")))
                .debug_selector(sel(if loading { "loading" } else { "image" }))
                .role(Role::Image)
                .aria_label(self.aria.clone())
                .cursor(CursorStyle::OpenHand)
                .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                    let mut shared = down_shared.borrow_mut();
                    if event.click_count >= 2 {
                        shared.fit();
                        shared.drag = None;
                    } else {
                        let at = [event.position.x.as_f32(), event.position.y.as_f32()];
                        shared.drag = Some((at, [shared.view.x, shared.view.y]));
                    }
                    drop(shared);
                    down_state.update(cx, |_, cx| cx.notify());
                    cx.stop_propagation();
                })
                .on_scroll_wheel(move |event, _, cx| {
                    let delta = event.delta.pixel_delta(px(20.));
                    let (dx, dy) = (delta.x.as_f32(), delta.y.as_f32());
                    let zoom = fullscreen || event.modifiers.secondary();
                    if !zoom && dx.abs() <= dy.abs() {
                        // Plain vertical scroll belongs to the chat.
                        return;
                    }
                    {
                        let mut shared = wheel_shared.borrow_mut();
                        shared.touched = true;
                        if zoom {
                            let [x, y] = shared.local(event.position);
                            shared.view.zoom_at(wheel_factor(dy), x, y);
                        } else {
                            shared.view.pan(-dx, 0.);
                        }
                    }
                    wheel_state.update(cx, |_, cx| cx.notify());
                    cx.stop_propagation();
                })
                .on_pinch(move |event, _, cx| {
                    {
                        let mut shared = pinch_shared.borrow_mut();
                        let [x, y] = shared.local(event.position);
                        shared.touched = true;
                        shared.view.zoom_at(pinch_factor(event.delta), x, y);
                    }
                    pinch_state.update(cx, |_, cx| cx.notify());
                    cx.stop_propagation();
                })
                .child(board)
                .into_any_element()
        };

        let escape_state = state.clone();
        let panel = div()
            .id(ElementId::from(self.id.clone()))
            .track_focus(&focus)
            .on_key_down(move |event, _, cx| {
                if event.keystroke.key == "escape" && escape_state.read(cx).fullscreen {
                    escape_state.update(cx, |s, cx| s.set_fullscreen(false, cx));
                    cx.stop_propagation();
                }
            })
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .rounded(px(10.))
            .overflow_hidden()
            .border_1()
            .border_color(content(0.1))
            .bg(if fullscreen {
                floating_fill()
            } else {
                content(0.06)
            })
            .child(toolbar)
            .child(
                div()
                    .w_full()
                    .debug_selector(sel("panel"))
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(content(0.1))
                    // Outside fullscreen the board's own height sizes the body.
                    .when(fullscreen, |d| d.flex_1().min_h_0())
                    .when_some(
                        if show_source {
                            self.source_status.or(self.status)
                        } else {
                            self.status
                        },
                        |d, status| {
                            d.child(
                                div()
                                    .px_3()
                                    .py_1()
                                    .flex_shrink_0()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(content(0.5))
                                    .child(status),
                            )
                        },
                    )
                    .child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(media),
                    ),
            );
        if !fullscreen {
            return panel.into_any_element();
        }
        let viewport = window.viewport_size();
        let placeholder = div().w_full().h(px(board_h + 43.));
        div()
            .child(placeholder)
            .child(
                gpui::deferred(
                    gpui::anchored().position(point(px(0.), px(0.))).child(
                        div()
                            .relative()
                            .w(viewport.width)
                            .h(viewport.height)
                            .occlude()
                            .bg(Hsla::black().opacity(0.55))
                            .child(
                                div()
                                    .absolute()
                                    .top(px(16.))
                                    .left(px(16.))
                                    .right(px(16.))
                                    .bottom(px(16.))
                                    .flex()
                                    .child(panel),
                            ),
                    ),
                )
                .with_priority(95),
            )
            .into_any_element()
    }
}

fn paint_board(
    bounds: Bounds<Pixels>,
    shared: &Rc<RefCell<Shared>>,
    state: &Entity<ViewerState>,
    content_data: &ViewerContent,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let refit = {
        let mut s = shared.borrow_mut();
        s.origin = [bounds.origin.x.as_f32(), bounds.origin.y.as_f32()];
        let board = [bounds.size.width.as_f32(), bounds.size.height.as_f32()];
        let resized = board != s.board;
        s.board = board;
        let first = !s.fitted && board[0] > 1. && board[1] > 1.;
        // Keep a user's own pan and zoom on resize; refit only the untouched view.
        if first || (resized && !s.touched) {
            s.fit();
        }
        first
    };
    if refit {
        // The zoom label was drawn before this paint fitted the view.
        let state = state.clone();
        cx.defer(move |cx| state.update(cx, |_, cx| cx.notify()));
    }
    // Esc needs focus in the panel; take it back if something else holds it.
    if state.read(cx).fullscreen && !focus.contains_focused(window, cx) {
        let focus = focus.clone();
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
    }
    // Drag follows the pointer anywhere in the window.
    let move_shared = shared.clone();
    let move_state = state.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase != DispatchPhase::Capture {
            return;
        }
        let mut s = move_shared.borrow_mut();
        if let Some((from, offset)) = s.drag {
            if event.pressed_button != Some(MouseButton::Left) {
                s.drag = None;
                return;
            }
            s.touched = true;
            s.view.x = offset[0] + event.position.x.as_f32() - from[0];
            s.view.y = offset[1] + event.position.y.as_f32() - from[1];
            drop(s);
            move_state.update(cx, |_, cx| cx.notify());
        }
    });
    let up_shared = shared.clone();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, _| {
        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
            up_shared.borrow_mut().drag = None;
        }
    });

    let view = shared.borrow().view;
    window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
        paint_dots(bounds, &view, window);
        let origin = bounds.origin + point(px(view.x), px(view.y));
        match content_data {
            ViewerContent::Flow(scene) => paint_flow(scene, view.scale, origin, window, cx),
            ViewerContent::Image(image) => {
                let [w, h] = content_data.size();
                let image_bounds =
                    Bounds::new(origin, size(px(w * view.scale), px(h * view.scale)));
                let _ = window.paint_image(
                    bounds,
                    image_bounds,
                    gpui::Corners::default(),
                    image.clone(),
                    0,
                    false,
                );
            }
            ViewerContent::Loading | ViewerContent::Empty => {}
        }
    });
}

fn paint_dots(bounds: Bounds<Pixels>, view: &Viewport, window: &mut Window) {
    window.paint_quad(gpui::fill(bounds, content(0.035)));
    let mut spacing = DOT_SPACING * view.scale;
    while spacing < 9. {
        spacing *= 2.;
    }
    let dot = content(0.2);
    let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    let start_x = view.x.rem_euclid(spacing);
    let start_y = view.y.rem_euclid(spacing);
    let mut y = start_y;
    while y < h {
        let mut x = start_x;
        while x < w {
            window.paint_quad(quad(
                Bounds::new(
                    bounds.origin + point(px(x - 0.8), px(y - 0.8)),
                    size(px(1.6), px(1.6)),
                ),
                px(0.8),
                dot,
                Edges::all(px(0.)),
                gpui::transparent_black(),
                gpui::BorderStyle::Solid,
            ));
            x += spacing;
        }
        y += spacing;
    }
}

fn paint_flow(
    scene: &FlowScene,
    scale: f32,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let line_color = content(0.62);
    let at = |x: f32, y: f32| origin + point(px(x * scale), px(y * scale));
    for edge in &scene.layout.edges {
        let spec = &scene.graph.edges[edge.index];
        let rounded = smooth(&edge.points, EDGE_RADIUS);
        let width = 1.6 * scale;
        if spec.dashed {
            for dash in dash_polyline(&rounded, 7., 5.) {
                paint_path(&dash, origin, scale, width, false, line_color, window);
            }
        } else {
            paint_path(&rounded, origin, scale, width, false, line_color, window);
        }
        if spec.arrow && edge.points.len() >= 2 {
            let n = edge.points.len();
            let head = arrowhead(edge.points[n - 1], edge.points[n - 2], ARROW);
            paint_path(&head, origin, scale, 0., true, line_color, window);
        }
    }
    for node in &scene.layout.nodes {
        paint_node(scene, node, scale, origin, window, cx);
    }
    for edge in &scene.layout.edges {
        let Some(label) = &edge.label else { continue };
        let r = label.rect;
        let b = Bounds::new(at(r.x, r.y), size(px(r.w * scale), px(r.h * scale)));
        window.paint_quad(quad(
            b,
            px(4. * scale),
            floating_fill(),
            Edges::all(px(1.)),
            content(0.2),
            gpui::BorderStyle::Solid,
        ));
        paint_centered(
            &label.text,
            at(r.x + r.w / 2., r.y + r.h / 2.),
            EDGE_FONT * scale,
            content(0.95),
            window,
            cx,
        );
    }
}

fn paint_node(
    scene: &FlowScene,
    node: &diagram::PlacedNode,
    scale: f32,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let spec = &scene.graph.nodes[node.index];
    let r = node.rect;
    let at = |x: f32, y: f32| origin + point(px(x * scale), px(y * scale));
    let bounds = Bounds::new(at(r.x, r.y), size(px(r.w * scale), px(r.h * scale)));
    let accent = spec.color.as_ref().map(color);
    let stroke = accent.unwrap_or(content(0.6));
    let tint = accent.map_or(content(0.05), |c| c.opacity(0.16));
    let radius = match spec.kind {
        NodeKind::Box => 5.,
        NodeKind::Round => 12.,
        NodeKind::Start | NodeKind::End | NodeKind::Circle => r.h.max(r.w),
        NodeKind::Decision => 0.,
    } * scale;
    let radius = px(radius.min(bounds.size.width.as_f32().min(bounds.size.height.as_f32()) / 2.));
    if spec.kind == NodeKind::Decision {
        let (cx_, cy) = (r.x + r.w / 2., r.y + r.h / 2.);
        let diamond = [[cx_, r.y], [r.x + r.w, cy], [cx_, r.y + r.h], [r.x, cy]];
        paint_path(&diamond, origin, scale, 0., true, floating_fill(), window);
        paint_path(&diamond, origin, scale, 0., true, tint, window);
        let outline = [diamond[0], diamond[1], diamond[2], diamond[3], diamond[0]];
        paint_path(&outline, origin, scale, 1.5 * scale, false, stroke, window);
    } else {
        window.paint_quad(quad(
            bounds,
            radius,
            floating_fill(),
            Edges::all(px(0.)),
            gpui::transparent_black(),
            gpui::BorderStyle::Solid,
        ));
        window.paint_quad(quad(
            bounds,
            radius,
            tint,
            Edges::all(px(1.5 * scale)),
            stroke,
            gpui::BorderStyle::Solid,
        ));
    }
    let lines = node.lines.len() as f32;
    let top = r.y + (r.h - lines * LINE_HEIGHT) / 2.;
    for (i, line) in node.lines.iter().enumerate() {
        paint_centered(
            line,
            at(r.x + r.w / 2., top + (i as f32 + 0.5) * LINE_HEIGHT),
            NODE_FONT * scale,
            content(1.),
            window,
            cx,
        );
    }
}

/// Paint `text` in the monospace font centred on `centre`.
fn paint_centered(
    text: &str,
    centre: Point<Pixels>,
    font_px: f32,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    if text.is_empty() || font_px < 3.5 {
        return;
    }
    let mut font = window.text_style().font();
    font.family = crate::fonts::mono().into();
    let run = gpui::TextRun {
        len: text.len(),
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window
        .text_system()
        .shape_line(text.to_owned().into(), px(font_px), &[run], None);
    let line_height = px(font_px * 1.4);
    let at = point(centre.x - line.width / 2., centre.y - line_height / 2.);
    let _ = line.paint(at, line_height, gpui::TextAlign::Left, None, window, cx);
}

#[cfg(test)]
mod tests;
