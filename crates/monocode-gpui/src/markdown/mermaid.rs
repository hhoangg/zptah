//! Native Mermaid previews with a bounded cache shared across virtualized rows.
use super::*;
use crate::diagram_viewer::{DiagramViewer, FlowScene, ViewerContent};
use gpui::{Context, IntoElement, RenderImage, RenderOnce, Subscription, Task};
use mermaid_rs_renderer::{LayoutConfig, Theme, compute_layout, parse_mermaid_strict, render_svg};
use monocode_core::diagram::mermaid::parse_flowchart;
use std::{collections::VecDeque, time::Duration};
#[path = "mermaid_curves.rs"]
mod curves;
use curves::flowchart_curves;
#[path = "mermaid_input.rs"]
mod input;
#[path = "mermaid_sequence.rs"]
mod sequence;

const CACHE_BYTES: usize = 32 * 1024 * 1024;
const CACHE_ITEMS: usize = 32;
const MAX_SOURCE_BYTES: usize = 32 * 1024;
const MAX_SOURCE_LINES: usize = 512;
const MAX_GRAPH_ITEMS: usize = 256;
const MAX_IMAGE_SIDE: f32 = 2048.;

type PreviewResult = Result<Arc<RenderImage>, String>;
type PreviewKey = (SharedString, bool);

#[derive(Default)]
struct DiagramCache {
    entries: VecDeque<(PreviewKey, PreviewResult, usize)>,
    generation: u64,
}
impl gpui::Global for DiagramCache {}

pub(crate) fn generation(cx: &App) -> u64 {
    cx.try_global::<DiagramCache>()
        .map_or(0, |cache| cache.generation)
}

fn cached(key: &PreviewKey, cx: &mut App) -> Option<PreviewResult> {
    let cache = cx.default_global::<DiagramCache>();
    let ix = cache.entries.iter().position(|entry| &entry.0 == key)?;
    let entry = cache.entries.remove(ix).unwrap();
    let result = entry.1.clone();
    cache.entries.push_back(entry);
    Some(result)
}

fn remember(key: PreviewKey, result: PreviewResult, cx: &mut App) {
    let bytes = key.0.len()
        + result.as_ref().map_or(0, |image| {
            let size = image.size(0);
            size.width.0 as usize * size.height.0 as usize * 4
        });
    let cache = cx.default_global::<DiagramCache>();
    cache.generation = cache.generation.wrapping_add(1);
    if bytes > CACHE_BYTES {
        return;
    }
    let mut evicted = Vec::new();
    if let Some(ix) = cache.entries.iter().position(|entry| entry.0 == key)
        && let Some((_, Ok(image), _)) = cache.entries.remove(ix)
    {
        evicted.push(image);
    }
    cache.entries.push_back((key, result, bytes));
    while cache.entries.len() > CACHE_ITEMS
        || cache.entries.iter().map(|entry| entry.2).sum::<usize>() > CACHE_BYTES
    {
        if let Some((_, Ok(image), _)) = cache.entries.pop_front() {
            evicted.push(image);
        }
    }
    for image in evicted {
        cx.drop_image(image, None);
        // Other panes may still be painting the same image; redraw them so
        // they upload it again instead of referring to an evicted sprite.
        cx.refresh_windows();
    }
}

#[derive(IntoElement)]
pub(super) struct Diagram {
    pub id: SharedString,
    pub fence: Arc<Fence>,
    pub selection: Option<TextSelection>,
}

#[derive(Default)]
struct Preview {
    key: Option<PreviewKey>,
    result: Option<PreviewResult>,
    /// The flowchart parsed into the shared diagram model, per source. `None`
    /// scene means the source is not a flowchart the shared parser covers, and
    /// the general Mermaid renderer draws it as an image.
    flow: Option<(SharedString, Option<Arc<FlowScene>>)>,
    task: Option<Task<()>>,
    _release: Option<Subscription>,
}

impl Preview {
    fn flow(&mut self, code: &SharedString) -> Option<Arc<FlowScene>> {
        if self.flow.as_ref().is_none_or(|(key, _)| key != code) {
            let scene = (code.len() <= MAX_SOURCE_BYTES
                && code.lines().count() <= MAX_SOURCE_LINES)
                .then(|| parse_flowchart(code).ok())
                .flatten()
                .and_then(|graph| FlowScene::new(graph).ok());
            self.flow = Some((code.clone(), scene));
        }
        self.flow.as_ref().and_then(|(_, scene)| scene.clone())
    }

    fn clear_image(&mut self, cx: &mut App) {
        if let Some(Ok(image)) = self.result.take() {
            let retained = cx.try_global::<DiagramCache>().is_some_and(|cache| {
                cache
                    .entries
                    .iter()
                    .any(|entry| matches!(&entry.1, Ok(cached) if cached.id == image.id))
            });
            if !retained {
                cx.drop_image(image, None);
                cx.refresh_windows();
            }
        }
    }

    fn prepare(&mut self, source: SharedString, dark: bool, cx: &mut Context<Self>) {
        if self.key.as_ref() == Some(&(source.clone(), dark)) {
            return;
        }
        self.task = None;
        self.clear_image(cx);
        let key = (source.clone(), dark);
        self.key = Some(key.clone());
        if self._release.is_none() {
            self._release = Some(cx.on_release(|this, cx| this.clear_image(cx)));
        }
        if let Some(result) = cached(&key, cx) {
            self.result = Some(result);
            return;
        }
        let renderer = cx.svg_renderer();
        let executor = cx.background_executor().clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            // Dropping this task on the next source edit cancels the debounce.
            // Completed work belongs only to this revision and theme.
            executor.timer(Duration::from_millis(150)).await;
            let result = executor
                .spawn(async move { rasterize(&source, dark, &renderer) })
                .await;
            let _ = this.update(cx, |this, cx| {
                remember(key, result.clone(), cx);
                this.result = Some(result);
                this.task = None;
                cx.notify();
            });
        }));
    }
}

fn diagram_svg(source: &str, dark: bool) -> Result<(String, f32, f32), String> {
    if source.len() > MAX_SOURCE_BYTES || source.lines().count() > MAX_SOURCE_LINES {
        return Err("Diagram is too large to preview. You can still copy its source.".into());
    }
    input::expansion_budget(source)?;
    let mut parsed = parse_mermaid_strict(source).map_err(|error| error.to_string())?;
    input::validate_styles(&parsed.graph)?;
    if parsed.graph.nodes.len() + parsed.graph.edges.len() > MAX_GRAPH_ITEMS {
        return Err("Diagram has too many nodes or connections to preview.".into());
    }
    if parsed.graph.kind == mermaid_rs_renderer::ir::DiagramKind::Sequence {
        for node in parsed.graph.nodes.values_mut() {
            node.label = sequence::wrap_participant(&node.label);
        }
    }
    let mut theme = if dark { Theme::dark() } else { Theme::modern() };
    theme.background = "transparent".into();
    theme.font_family = format!("{}, monospace", crate::fonts::mono());
    let config = LayoutConfig::default();
    let layout = compute_layout(&parsed.graph, &theme, &config);
    if let mermaid_rs_renderer::layout::DiagramData::Error(error) = &layout.diagram {
        return Err(error.message.clone());
    }
    let dimensions = mermaid_rs_renderer::measure_svg_dimensions(&layout, &config, None);
    let width = dimensions.width;
    let height = dimensions.height;
    if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
        return Err("Diagram has invalid dimensions.".into());
    }
    let mut svg = render_svg(&layout, &theme, &config);
    if layout.kind == mermaid_rs_renderer::ir::DiagramKind::Flowchart {
        flowchart_curves(&mut svg, &layout);
    }
    Ok((svg, width, height))
}

fn rasterize(
    source: &str,
    dark: bool,
    renderer: &gpui::SvgRenderer,
) -> Result<Arc<RenderImage>, String> {
    let (svg, width, height) = diagram_svg(source, dark)?;
    let scale = (MAX_IMAGE_SIDE / width.max(height)).min(2.);
    let svg = renderer
        .parse_svg_without_images(svg.as_bytes())
        .map_err(|error| error.to_string())?;
    renderer
        .render_parsed(
            &svg,
            gpui::SvgSize::ExactSize(gpui::size(
                gpui::DevicePixels((width * scale).ceil().clamp(1., MAX_IMAGE_SIDE) as i32),
                gpui::DevicePixels((height * scale).ceil().clamp(1., MAX_IMAGE_SIDE) as i32),
            )),
        )
        .map_err(|error| error.to_string())
}

fn source_preview(fence: &Arc<Fence>) -> Arc<Fence> {
    let mut end = fence.code.len().min(MAX_SOURCE_BYTES);
    if let Some(line) = fence.lines.get(MAX_SOURCE_LINES) {
        end = end.min(line.start);
    }
    while !fence.code.is_char_boundary(end) {
        end -= 1;
    }
    if end == fence.code.len() {
        fence.clone()
    } else {
        Arc::new(Fence::new("mermaid", &fence.code[..end]))
    }
}

impl RenderOnce for Diagram {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            SharedString::from(format!("{}-mermaid-state", self.id)),
            cx,
            |_, _| Preview::default(),
        );
        let code = self.fence.code.clone();
        let flow = state.update(cx, |state, _| state.flow(&code));
        let (content, status, error) = if let Some(scene) = flow {
            (ViewerContent::Flow(scene), None, false)
        } else {
            state.update(cx, |state, cx| state.prepare(code.clone(), !is_light(), cx));
            match &state.read(cx).result {
                None => (
                    ViewerContent::Loading,
                    Some("Rendering diagram…".to_owned()),
                    false,
                ),
                Some(Err(error)) => (
                    ViewerContent::Empty,
                    Some(format!("Unable to preview Mermaid: {error}")),
                    true,
                ),
                Some(Ok(image)) => (ViewerContent::Image(image.clone()), None, false),
            }
        };
        let fence = source_preview(&self.fence);
        let truncated = !Arc::ptr_eq(&fence, &self.fence);
        let selection = self.selection;
        let source = Box::new(move || {
            div()
                .id("mermaid-source")
                .size_full()
                .overflow_scroll()
                .py_2()
                .child(code_lines_selected(
                    &fence,
                    0..fence.lines.len(),
                    None,
                    "",
                    false,
                    selection,
                ))
                .into_any_element()
        });
        let mut viewer = DiagramViewer::new(self.id, "mermaid", self.fence.label.clone(), content)
            .icon("code.mermaid")
            .aria(format!("Mermaid diagram\n{}", self.fence.code))
            .source(self.fence.code.clone(), source)
            .force_source(error)
            .status(status);
        if truncated {
            viewer = viewer
                .source_status("Source preview is truncated. Copy includes the complete source.");
        }
        viewer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        visible: bool,
        width: f32,
        code: &'static str,
    }

    impl gpui::Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(self.width)).when(self.visible, |d| {
                d.child(Diagram {
                    id: "diagram-fixture".into(),
                    fence: Arc::new(Fence::new("mermaid", self.code)),
                    selection: None,
                })
            })
        }
    }

    fn open_fixture(
        cx: &mut gpui::TestAppContext,
        code: &'static str,
    ) -> (
        gpui::WindowHandle<Fixture>,
        &'static mut gpui::VisualTestContext,
    ) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| {
                cx.new(|_| Fixture {
                    visible: true,
                    width: 640.,
                    code,
                })
            })
            .unwrap()
        });
        let visual = gpui::VisualTestContext::from_window(window.into(), cx).into_mut();
        (window, visual)
    }

    #[gpui::test]
    fn raster_preview_buttons_switch_source_and_copy_without_changing_height(
        cx: &mut gpui::TestAppContext,
    ) {
        let code = "sequenceDiagram\n Alice->>Bob: Hello\n Bob-->>Alice: Hi";
        let (window, visual) = open_fixture(cx, code);
        visual.run_until_parked();
        assert!(visual.debug_bounds("mermaid-source").is_none());
        assert!(visual.debug_bounds("mermaid-loading").is_some());
        visual.executor().advance_clock(Duration::from_millis(200));
        visual.run_until_parked();
        let image = visual.debug_bounds("mermaid-image").unwrap();
        assert!(image.size.width > px(0.) && image.size.height > px(0.));
        let button = visual.debug_bounds("mermaid-toggle").unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("mermaid-source").unwrap().size.height,
            image.size.height
        );
        assert!(visual.debug_bounds("mermaid-image").is_none());
        let copy = visual.debug_bounds("mermaid-copy").unwrap();
        visual.simulate_click(copy.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            visual
                .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
                .as_deref(),
            Some(code)
        );
        let button = visual.debug_bounds("mermaid-toggle").unwrap();
        visual.simulate_click(button.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("mermaid-image").unwrap(), image);
        window
            .update(visual, |this, _, cx| {
                this.visible = false;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        window
            .update(visual, |this, _, cx| {
                this.visible = true;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        assert_eq!(visual.debug_bounds("mermaid-image").unwrap(), image);
        assert!(visual.debug_bounds("mermaid-source").is_none());
        assert!(visual.debug_bounds("mermaid-loading").is_none());
        // The board follows the container width; its height stays put.
        window
            .update(visual, |this, _, cx| {
                this.width = 400.;
                cx.notify();
            })
            .unwrap();
        visual.run_until_parked();
        let narrow = visual.debug_bounds("mermaid-image").unwrap();
        assert!(narrow.size.width < image.size.width);
        assert_eq!(narrow.size.height, image.size.height);
    }

    #[gpui::test]
    fn every_diagram_kind_fits_its_board_inside_the_panel(cx: &mut gpui::TestAppContext) {
        let (window, visual) = open_fixture(cx, "graph LR; A-->B");
        for code in [
            "sequenceDiagram\n Alice->>Bob: Hello\n Bob-->>Alice: Hi",
            "classDiagram\n Animal <|-- Duck\n Animal : +int age",
            "stateDiagram-v2\n [*] --> Ready\n Ready --> [*]",
            "erDiagram\n CUSTOMER ||--o{ ORDER : places",
            "graph TD; A-->B-->C-->D-->E-->F-->G-->H",
            "flowchart LR\n subgraph s\n A-->B\n end",
        ] {
            window
                .update(visual, |this, _, cx| {
                    this.code = code;
                    cx.notify();
                })
                .unwrap();
            visual.run_until_parked();
            assert!(visual.debug_bounds("mermaid-source").is_none());
            visual.executor().advance_clock(Duration::from_millis(200));
            visual.run_until_parked();
            for width in [400., 900., 600.] {
                window
                    .update(visual, |this, _, cx| {
                        this.width = width;
                        cx.notify();
                    })
                    .unwrap();
                visual.run_until_parked();
                let board = visual.debug_bounds("mermaid-image").unwrap();
                let panel = visual.debug_bounds("mermaid-panel").unwrap();
                assert!(
                    board.size.height <= px(crate::diagram_viewer::MAX_BOARD),
                    "{code}"
                );
                assert!(panel.contains(&board.origin), "{code}, width={width}");
                assert!(
                    board.bottom_right().x <= panel.bottom_right().x + px(0.1),
                    "{code}, width={width}"
                );
            }
        }
    }

    #[gpui::test]
    fn flowcharts_render_vectors_in_the_shared_viewer(cx: &mut gpui::TestAppContext) {
        let code = "flowchart LR\n A[Start] --> B{Ready?}\n B -->|Yes| C[Done]";
        let (_, visual) = open_fixture(cx, code);
        visual.run_until_parked();
        // No debounce and no rasterising: the board is there at once.
        assert!(visual.debug_bounds("mermaid-loading").is_none());
        let board = visual.debug_bounds("mermaid-image").unwrap();
        assert!(board.size.width > px(300.) && board.size.height >= px(180.));
        let toggle = visual.debug_bounds("mermaid-toggle").unwrap();
        visual.simulate_click(toggle.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            visual.debug_bounds("mermaid-source").unwrap().size.height,
            board.size.height
        );
        let copy = visual.debug_bounds("mermaid-copy").unwrap();
        visual.simulate_click(copy.center(), Default::default());
        visual.run_until_parked();
        assert_eq!(
            visual
                .update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()))
                .as_deref(),
            Some(code)
        );
    }

    #[test]
    fn renders_common_diagrams_in_both_themes() {
        for source in [
            "flowchart LR\n A[Start] --> B{Ready?}\n B -->|Yes| C[Done]",
            "sequenceDiagram\n Alice->>Bob: Hello\n Bob-->>Alice: Hi",
            "classDiagram\n Animal <|-- Duck\n Animal : +int age",
            "stateDiagram-v2\n [*] --> Ready\n Ready --> [*]",
            "erDiagram\n CUSTOMER ||--o{ ORDER : places",
        ] {
            for dark in [false, true] {
                let (svg, width, height) = diagram_svg(source, dark).unwrap();
                assert!(svg.contains("<svg"));
                assert!(width > 0. && height > 0.);
            }
        }
        assert_ne!(
            diagram_svg("graph TD; A-->B", false).unwrap().0,
            diagram_svg("graph TD; A-->B", true).unwrap().0
        );
        let svg = diagram_svg("graph TD; A-->B; C-->B; D-->B", true)
            .unwrap()
            .0;
        assert!(svg.contains("class=\"edgePath\""));
        assert!(
            svg.contains(" C "),
            "flowchart arrows should use smooth basis curves"
        );
        assert!(
            !svg.contains("fill=\"#333333\""),
            "background should blend into the panel"
        );
    }

    #[test]
    fn invalid_and_oversized_diagrams_return_errors() {
        assert!(diagram_svg("not a diagram", false).is_err());
        assert!(diagram_svg(&"x".repeat(MAX_SOURCE_BYTES + 1), false).is_err());
        assert!(diagram_svg(&"\n".repeat(MAX_SOURCE_LINES + 1), false).is_err());
        let large = format!(
            "graph TD\n{}",
            (0..MAX_GRAPH_ITEMS)
                .map(|n| format!(" A{n}-->A{}\n", n + 1))
                .collect::<String>()
        );
        assert!(diagram_svg(&large, false).is_err());
        let original = Arc::new(Fence::new("mermaid", &"é\n".repeat(MAX_SOURCE_BYTES)));
        let display = source_preview(&original);
        assert!(display.code.len() <= MAX_SOURCE_BYTES);
        assert!(display.lines.len() <= MAX_SOURCE_LINES);
        assert!(original.code.starts_with(display.code.as_ref()));
        assert_eq!(original.code.len(), MAX_SOURCE_BYTES * 3);
    }

    #[test]
    fn grouped_endpoint_expansion_is_rejected_before_parsing() {
        let group = |prefix: &str, count: usize| {
            (0..count)
                .map(|ix| format!("{prefix}{ix}"))
                .collect::<Vec<_>>()
                .join(" & ")
        };
        let bomb = format!("graph TD\n{} --> {}", group("A", 1000), group("B", 1000));
        assert!(bomb.len() < 15 * 1024);
        assert!(
            diagram_svg(&bomb, false)
                .unwrap_err()
                .contains("parser expansion budget")
        );
        for arrow in ["-.->", "==>", "<-->", "-- label -->", "-- \"label\" -->"] {
            let source = format!(
                "graph TD\n{} {arrow} {}",
                group("A", 1000),
                group("B", 1000)
            );
            assert!(
                diagram_svg(&source, false)
                    .unwrap_err()
                    .contains("parser expansion budget")
            );
        }
        let block = bomb.replacen("graph TD", "block-beta", 1);
        assert!(
            diagram_svg(&block, false)
                .unwrap_err()
                .contains("parser expansion budget")
        );
        let classes = format!(
            "graph TD\n class {} {}",
            group("A", 1000).replace(" & ", ","),
            group("C", 1000).replace(" & ", ",")
        );
        assert!(
            diagram_svg(&classes, false)
                .unwrap_err()
                .contains("parser expansion budget")
        );
        let styles = format!("graph TD\n style A fill:{}", "a".repeat(2049));
        assert!(
            diagram_svg(&styles, false)
                .unwrap_err()
                .contains("style command")
        );
        let chain = format!(
            "graph TD\n{} -->|label| {} --> {}",
            group("A", 100),
            group("B", 100),
            group("C", 100)
        );
        assert!(
            diagram_svg(&chain, false)
                .unwrap_err()
                .contains("parser expansion budget")
        );
        // Small Cartesian groups, chains, and linear fanout remain usable.
        for source in [
            "graph TD; A & B --> C & D".to_owned(),
            format!("graph TD; {} --> Z", group("A", 16)),
        ] {
            assert!(diagram_svg(&source, false).is_ok(), "{source}");
        }
    }

    #[test]
    #[cfg_attr(
        not(target_os = "macos"),
        ignore = "mermaid-rs-renderer 0.3.1 fails its own debug-only flowchart stage_validation assert (final_layout_errors: 5) on the 8x8x8 grouped chain (24 nodes, 128 edges) on Linux x86_64 and aarch64; its layout differs from macOS, and release builds skip the assert"
    )]
    fn a_small_grouped_chain_remains_usable() {
        let group = |prefix: &str, count: usize| {
            (0..count)
                .map(|ix| format!("{prefix}{ix}"))
                .collect::<Vec<_>>()
                .join(" & ")
        };
        let source = format!(
            "graph TD; {} --> {} --> {}",
            group("A", 8),
            group("B", 8),
            group("C", 8)
        );
        assert!(diagram_svg(&source, false).is_ok(), "{source}");
    }

    #[test]
    fn source_styles_cannot_inject_svg_attributes_or_elements() {
        let payload = "red\"/><image href=\"/tmp/harmless-mermaid-test.png\" width=\"2\" height=\"2\"/><rect fill=\"red";
        for directive in [
            format!("style A fill:{payload}"),
            format!("style A stroke:{payload}"),
            format!("style A color:{payload}"),
            format!("style A stroke-dasharray:{payload}"),
            format!("classDef unsafe fill:{payload}"),
            format!("linkStyle default stroke:{payload}"),
            "style A fill:url(file:///tmp/harmless-mermaid-test.png)".into(),
            "style A stroke-width:NaN".into(),
        ] {
            let source = format!("graph TD\n A --> B\n{directive}");
            assert!(
                diagram_svg(&source, false)
                    .unwrap_err()
                    .contains("unsafe style"),
                "{directive}"
            );
        }
        let svg = diagram_svg("graph TD\n A[\"A & B\"] --> B\n style A fill:#aabbcc,stroke:red,stroke-width:2,color:blue", false).unwrap().0;
        assert!(svg.contains("#aabbcc"));
        assert!(svg.contains("A &amp; B"));
    }

    #[test]
    fn mermaid_svg_policy_does_not_resolve_a_harmless_local_image() {
        use base64::Engine as _;
        let root = std::env::temp_dir().join(format!(
            "mermaid-image-policy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("harmless.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let path = path
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('"', "&quot;");
        let injected = format!(
            "graph TD\n A-->B\n style A fill:red\"/><image href=\"{path}\" width=\"2\" height=\"2\"/><rect fill=\"red"
        );
        assert!(diagram_svg(&injected, false).is_err());
        let file_svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="2" height="2"><image xlink:href="{path}" width="2" height="2"/></svg>"#
        );
        // usvg already blocks external files inside a nested SVG. Include a
        // visible vector control to prove our stricter policy rejects the
        // entire data image rather than merely inheriting that protection.
        let embedded_svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red"/><image href="{path}" width="2" height="2"/></svg>"#
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(embedded_svg.as_bytes());
        let nested_svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image href="data:image/svg+xml;base64,{encoded}" width="2" height="2"/></svg>"#
        );
        let renderer = gpui::SvgRenderer::new(Arc::new(()));
        for svg in [file_svg, nested_svg] {
            let size =
                gpui::SvgSize::ExactSize(gpui::size(gpui::DevicePixels(2), gpui::DevicePixels(2)));
            let normal = renderer
                .render_parsed(&renderer.parse_svg(svg.as_bytes()).unwrap(), size)
                .unwrap();
            assert!(
                normal
                    .as_bytes(0)
                    .unwrap()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[3] != 0),
                "control must load the harmless image"
            );
            let blocked = renderer
                .render_parsed(
                    &renderer.parse_svg_without_images(svg.as_bytes()).unwrap(),
                    size,
                )
                .unwrap();
            assert!(
                blocked.as_bytes(0).unwrap().iter().all(|&byte| byte == 0),
                "Mermaid policy must not load direct or nested images"
            );
        }
        std::fs::remove_file(root.join("harmless.png")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn mermaid_is_one_row_and_keeps_source_and_container_context() {
        let source = format!("graph TD\n{}", " A-->B\n".repeat(PART_LINES * 2));
        let block = Block::BlockQuote {
            children: vec![Block::CodeBlock {
                language: Some("mermaid title=diagram.mmd".into()),
                code: source.clone(),
            }],
        };
        let rows = rows(&block);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].quote, 1);
        assert_eq!(selection_text(&rows[0]), source);
        assert!(matches!(rows[0].part, Part::Mermaid { .. }));
        assert_eq!(
            super::super::rows(&Block::CodeBlock {
                language: Some("rust".into()),
                code: "line\n".repeat(PART_LINES * 2),
            })
            .len(),
            2
        );
    }

    #[test]
    fn streamed_fences_preserve_literal_source_before_and_after_closing() {
        let code = "flowchart TD\n A[\"café **literal**\"] --> B\n";
        let open = format!("```mermaid\n{code}");
        let mut parser = crate::markdown_model::MarkdownParser::new();
        for text in [open.clone(), format!("{open}```\n")] {
            parser.set_text(&text);
            let tree = parser.display_tree();
            let row = rows(&tree.blocks[0].block).remove(0);
            assert!(matches!(row.part, Part::Mermaid { .. }));
            assert_eq!(row_text(&row), code);
        }
    }

    #[gpui::test]
    fn source_updates_cancel_pending_work_and_theme_changes_replace_pixels(
        cx: &mut gpui::TestAppContext,
    ) {
        let preview = cx.new(|_| Preview::default());
        preview.update(cx, |this, cx| {
            this.prepare("not a diagram".into(), false, cx)
        });
        cx.run_until_parked();
        preview.update(cx, |this, cx| {
            this.prepare("graph TD; A-->B".into(), false, cx)
        });
        cx.run_until_parked();
        preview.read_with(cx, |this, _| assert!(this.result.is_none()));
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        let first = preview.read_with(cx, |this, _| {
            this.result.as_ref().unwrap().as_ref().unwrap().clone()
        });
        let size = first.size(0);
        assert!(size.width.0 > 0 && size.height.0 > 0);
        assert!(size.width.0 <= MAX_IMAGE_SIDE as i32 && size.height.0 <= MAX_IMAGE_SIDE as i32);
        preview.update(cx, |this, cx| {
            this.prepare("graph TD; A-->B".into(), false, cx)
        });
        preview.read_with(cx, |this, _| {
            assert!(Arc::ptr_eq(
                this.result.as_ref().unwrap().as_ref().unwrap(),
                &first
            ))
        });
        preview.update(cx, |this, cx| {
            this.prepare("graph TD; A-->B".into(), true, cx)
        });
        preview.read_with(cx, |this, _| assert!(this.result.is_none()));
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        preview.read_with(cx, |this, _| {
            assert!(!Arc::ptr_eq(
                this.result.as_ref().unwrap().as_ref().unwrap(),
                &first
            ))
        });
        preview.update(cx, |this, cx| {
            this.prepare("not a diagram".into(), true, cx)
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        preview.read_with(cx, |this, _| assert!(matches!(this.result, Some(Err(_)))));
    }

    #[gpui::test]
    fn cache_reuses_pixels_after_row_release_and_bounds_errors(cx: &mut gpui::TestAppContext) {
        let preview = cx.new(|_| Preview::default());
        preview.update(cx, |this, cx| {
            this.prepare("graph LR; A-->B".into(), false, cx)
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        let image = preview.read_with(cx, |this, _| {
            this.result.as_ref().unwrap().as_ref().unwrap().clone()
        });
        drop(preview);
        cx.run_until_parked();
        let remounted = cx.new(|_| Preview::default());
        remounted.update(cx, |this, cx| {
            this.prepare("graph LR; A-->B".into(), false, cx);
            assert!(this.task.is_none());
            assert!(Arc::ptr_eq(
                this.result.as_ref().unwrap().as_ref().unwrap(),
                &image
            ));
        });
        cx.update(|cx| {
            for n in 0..CACHE_ITEMS * 3 {
                remember(
                    (format!("invalid-{n}").into(), false),
                    Err("Invalid syntax".into()),
                    cx,
                );
            }
            assert_eq!(cx.global::<DiagramCache>().entries.len(), CACHE_ITEMS);
            assert!(cached(&("graph LR; A-->B".into(), false), cx).is_none());
            for n in 0..10 {
                let pixels = image::RgbaImage::new(1024, 1024);
                let image = Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
                remember((format!("image-{n}").into(), false), Ok(image), cx);
            }
            assert!(
                cx.global::<DiagramCache>()
                    .entries
                    .iter()
                    .map(|entry| entry.2)
                    .sum::<usize>()
                    <= CACHE_BYTES
            );
        });
    }
}
