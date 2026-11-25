// The canvas and flow nodes of a visual tree (crates/monocode-core/src/visual/model.rs): label anchoring, rect radius
// and stroke, arrows and dashes, chips, automatic ink, and the flow diagram in the shared viewer. All drawn from data.
import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { ASCENT } from "@/features/diagram/model";
import { useAppearance } from "@/lib/appearance";
import { INK_DARK, INK_LIGHT, inkOn, labelBackdrop, tokenRgb } from "./ink";
import { parseVisualTree } from "./parse";
import type { Shape, VisualNode } from "./types";
import { VisualTree } from "./VisualTree";

const rect = (over: Partial<Extract<Shape, { kind: "rect" }>> = {}): Shape => ({ kind: "rect", x: 0, y: 0, width: 100, height: 40, color: "warning", radius: 0, stroke: null, stroke_width: 1.5, fill: true, ...over });
const label = (over: Partial<Extract<Shape, { kind: "label" }>> = {}): Shape => ({ kind: "label", x: 10, y: 20, text: "hi", color: null, align: "start", baseline: "top", chip: false, ...over });
const canvas = (shapes: Shape[]): VisualNode => ({ kind: "canvas", width: 320, height: 160, shapes });
const drawn = (shapes: Shape[]) => render(<VisualTree node={canvas(shapes)} />).container.querySelector("svg[data-visual='canvas']") as SVGSVGElement;

beforeEach(() => useAppearance.setState({ scheme: "dark" }));

describe("canvas shapes", () => {
  it("draws rects with a radius clamped to the half size, a stroke, and no fill", () => {
    const svg = drawn([rect({ radius: 500, stroke: "accent", stroke_width: 3, fill: false })]);
    const r = svg.querySelector("rect")!;
    expect(r.getAttribute("rx")).toBe("20");
    expect(r.getAttribute("fill")).toBe("none");
    expect(r.getAttribute("stroke-width")).toBe("3");
    expect(r.getAttribute("stroke")).toBeTruthy();
    expect(drawn([rect()]).querySelector("rect")!.getAttribute("stroke")).toBeNull();
  });

  it("draws circles with an optional stroke and fill", () => {
    const svg = drawn([{ kind: "circle", x: 50, y: 50, radius: 10, color: "positive", stroke: "ink", stroke_width: 2, fill: false }]);
    const c = svg.querySelector("circle")!;
    expect(c.getAttribute("fill")).toBe("none");
    expect(c.getAttribute("stroke-width")).toBe("2");
  });

  it("draws arrowheads on the requested ends and dashes on request", () => {
    const line = (arrow: "none" | "end" | "start" | "both", dashed = false): Shape => ({ kind: "line", x1: 0, y1: 0, x2: 50, y2: 0, width: 2, color: "ink", arrow, dashed });
    expect(drawn([line("none")]).querySelectorAll("polygon")).toHaveLength(0);
    expect([...drawn([line("end")]).querySelectorAll("polygon")].map((p) => p.dataset["arrow"])).toEqual(["end"]);
    expect([...drawn([line("start")]).querySelectorAll("polygon")].map((p) => p.dataset["arrow"])).toEqual(["start"]);
    expect([...drawn([line("both")]).querySelectorAll("polygon")].map((p) => p.dataset["arrow"])).toEqual(["end", "start"]);
    expect(drawn([line("none", true)]).querySelector("line")!.getAttribute("stroke-dasharray")).toBe("12 8");
    const poly = drawn([{ kind: "polyline", points: [[0, 0], [10, 10], [20, 0]], width: 1, color: "accent", arrow: "end", dashed: true }]);
    expect(poly.querySelector("polyline")!.getAttribute("stroke-dasharray")).toBe("6 4");
    expect(poly.querySelectorAll("polygon")).toHaveLength(1);
  });

  it("anchors labels by align and baseline", () => {
    const text = (over: Partial<Extract<Shape, { kind: "label" }>>) => drawn([label(over)]).querySelector("text")!;
    // The defaults keep (x, y) the top-left: the baseline sits one ascent below.
    const plain = text({});
    expect([plain.getAttribute("x"), plain.getAttribute("y"), plain.getAttribute("text-anchor")]).toEqual(["10", String(20 + ASCENT * 12), "start"]);
    expect(text({ align: "center" }).getAttribute("text-anchor")).toBe("middle");
    expect(text({ align: "end" }).getAttribute("text-anchor")).toBe("end");
    expect(Number(text({ baseline: "middle" }).getAttribute("y"))).toBeCloseTo(20 - 8.4 + 12, 6);
    expect(Number(text({ baseline: "bottom" }).getAttribute("y"))).toBeCloseTo(20 - 16.8 + 12, 6);
    expect(Number(text({ baseline: "alphabetic" }).getAttribute("y"))).toBeCloseTo(20, 6);
  });

  it("paints a chip behind a label on request", () => {
    expect(drawn([label({ chip: true })]).querySelector("rect[data-chip]")).not.toBeNull();
    expect(drawn([label({})]).querySelector("rect[data-chip]")).toBeNull();
  });

  it("picks dark or light ink from the fill under an automatic label", () => {
    const ink = (shapes: Shape[], index: number) => drawn(shapes).querySelectorAll("text")[index]!.getAttribute("fill");
    // Warning is a light orange: dark ink. A deep violet in the light scheme: light ink.
    expect(ink([rect({ color: "warning" }), label({ x: 50, y: 20 })], 0)).toBe(INK_DARK);
    useAppearance.setState({ scheme: "light" });
    expect(ink([rect({ color: "series3" }), label({ x: 50, y: 20 })], 0)).toBe(INK_LIGHT);
    // No fill under the anchor: plain ink; an explicit colour always wins.
    expect(ink([rect({ color: "series3" }), label({ x: 500, y: 20 })], 0)).toBe("var(--content)");
    expect(ink([rect({ color: "series3" }), label({ x: 50, y: 20, color: "positive" })], 0)).not.toBe(INK_LIGHT);
  });

  it("finds the fill under a label like the desktop", () => {
    const shapes: Shape[] = [
      rect({ color: "warning" }),
      rect({ color: "accent", fill: false, stroke: "ink" }),
      { kind: "circle", x: 200, y: 20, radius: 10, color: "positive", stroke: null, stroke_width: 1.5, fill: true },
      label({ x: 50, y: 20, align: "center", baseline: "middle" }),
      label({ x: 200, y: 20 }),
      label({ x: 300, y: 300 }),
    ];
    expect(labelBackdrop(shapes, 3)).toBe("warning");
    expect(labelBackdrop(shapes, 4)).toBe("positive");
    expect(labelBackdrop(shapes, 5)).toBeNull();
    expect(labelBackdrop(shapes, 0)).toBeNull();
    expect(inkOn(tokenRgb("warning", "dark"))).toBe(INK_DARK);
    expect(inkOn(tokenRgb("series3", "light"))).toBe(INK_LIGHT);
  });

  it("keeps hostile label text as text", () => {
    const hostile = '<img src=x onerror="alert(1)"><script>alert(1)</script>';
    const { container } = render(<VisualTree node={canvas([label({ text: hostile, chip: true })])} />);
    expect(container.querySelector("img, script")).toBeNull();
    expect(container.querySelector("text")!.textContent).toBe(hostile);
  });
});

const flowTree = (over: Record<string, unknown> = {}) => ({
  kind: "flow",
  title: "Approval flow",
  height: null,
  graph: {
    direction: "LR",
    nodes: [
      { id: "a", label: "Request", kind: "start", color: "accent" },
      { id: "b", label: "Classify risk?", kind: "decision", color: null },
      { id: "c", label: "Auto act", kind: "box", color: "positive" },
    ],
    edges: [
      { from: "a", to: "b", label: null, dashed: false, arrow: true },
      { from: "b", to: "c", label: "safe", dashed: true, arrow: true },
    ],
  },
  ...over,
});

describe("flow node", () => {
  it("is read from the desktop's serialisation and drawn in the shared viewer", async () => {
    const node = parseVisualTree(flowTree());
    expect(node).toMatchObject({ kind: "flow", title: "Approval flow", height: null });
    render(<VisualTree node={node!} />);
    const viewer = await screen.findByTestId("visual-flow");
    expect(within(viewer).getByTestId("diagram-title")).toHaveTextContent("Approval flow");
    expect(viewer.querySelectorAll("[data-flow-node]")).toHaveLength(3);
    expect(viewer.querySelectorAll("[data-flow-edge]")).toHaveLength(2);
    expect(viewer.querySelector("[data-flow-edge][data-dashed]")).not.toBeNull();
    expect(viewer.querySelector("[data-kind='decision'] polygon")).not.toBeNull();
    expect(within(viewer).getByText("safe")).toBeInTheDocument();
    expect(within(viewer).getByRole("button", { name: "Fit to view" })).toBeInTheDocument();
    // A flow has no source: no Source or Copy.
    expect(within(viewer).queryByRole("button", { name: "Show source" })).toBeNull();
    expect(screen.getByRole("img", { name: "Approval flow diagram" })).toBeInTheDocument();
  });

  it("defaults the direction, the node kind and the arrow, and refuses a bad graph", () => {
    const minimal = parseVisualTree({ kind: "flow", graph: { nodes: [{ id: "a", label: "A" }, { id: "b", label: "B" }], edges: [{ from: "a", to: "b" }] } });
    expect(minimal).toMatchObject({ kind: "flow", title: null, graph: { direction: "LR", nodes: [{ kind: "box", color: null }, { kind: "box" }], edges: [{ arrow: true, dashed: false, label: null }] } });
    expect(parseVisualTree(flowTree({ graph: { nodes: [], edges: [] } }))).toBeNull();
    expect(parseVisualTree(flowTree({ graph: { nodes: [{ id: "a", label: "A" }], edges: [{ from: "a", to: "zzz" }] } }))).toBeNull();
    expect(parseVisualTree(flowTree({ graph: { nodes: [{ id: "a", label: "A" }, { id: "a", label: "B" }], edges: [] } }))).toBeNull();
    expect(parseVisualTree(flowTree({ graph: "x" }))).toBeNull();
    expect(parseVisualTree(flowTree({ graph: { nodes: Array.from({ length: 121 }, (_, i) => ({ id: `n${i}`, label: "x" })), edges: [] } }))).toBeNull();
  });

  it("renders hostile titles and labels as text", async () => {
    const hostile = '<img src=x onerror="alert(1)"><script>alert(1)</script>';
    const node = parseVisualTree(flowTree({ title: hostile, graph: { direction: "TB", nodes: [{ id: "a", label: hostile }, { id: "b", label: "B" }], edges: [{ from: "a", to: "b", label: hostile }] } }));
    const { container } = render(<VisualTree node={node!} />);
    await screen.findByTestId("visual-flow");
    expect(container.querySelector("img, script")).toBeNull();
    expect(screen.getByTestId("diagram-title").textContent).toBe(hostile);
    // The label wraps onto several lines; the lines together are the text, unchanged.
    expect(container.querySelector("[data-flow-node='a']")!.textContent!.replaceAll(" ", "")).toBe(hostile.replaceAll(" ", ""));
  });
});

describe("canvas shapes from the desktop", () => {
  it("reads the new fields and defaults the old ones", () => {
    const node = parseVisualTree({
      kind: "canvas",
      width: 300,
      height: 200,
      shapes: [
        { kind: "rect", x: 0, y: 0, width: 10, height: 10, color: "accent" },
        { kind: "label", x: 1, y: 2, text: "t", color: null, align: "center", baseline: "alphabetic", chip: true },
        { kind: "line", x1: 0, y1: 0, x2: 1, y2: 1, width: 1, color: "ink", arrow: "both", dashed: true },
      ],
    });
    expect(node).toMatchObject({
      shapes: [
        { kind: "rect", radius: 0, stroke: null, stroke_width: 1.5, fill: true },
        { kind: "label", color: null, align: "center", baseline: "alphabetic", chip: true },
        { kind: "line", arrow: "both", dashed: true },
      ],
    });
    expect(parseVisualTree({ kind: "canvas", width: 3, height: 3, shapes: [{ kind: "label", x: 0, y: 0, text: "t", align: "diagonal" }] })).toMatchObject({ shapes: [{ align: "start", baseline: "top", chip: false, color: null }] });
  });
});
