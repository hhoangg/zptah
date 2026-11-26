// F-73, F-74: the visual tree renderer draws every node kind as React elements and inline SVG, with no HTML sink.
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { ContributionGraph } from "./charts";
import { inked } from "./dither";
import { CHART_HEIGHT, MAX_DEPTH, MAX_NODES, parseVisualTree } from "./parse";
import { VisualTree } from "./VisualTree";
import type { VisualNode } from "./types";

const series = [{ label: "A", values: [1, 3, 2, 5] }, { label: "B", values: [2, 1, 4, 3], color: "series3" as const }];
const labels = ["Mon", "Tue", "Wed", "Thu"];

const KINDS: Record<string, VisualNode> = {
  bar: { kind: "chart", chart_type: "bar", series, labels, height: 200 },
  line: { kind: "chart", chart_type: "line", series, labels, height: 200 },
  area: { kind: "chart", chart_type: "area", series, labels, height: 200 },
  gauge: { kind: "gauge", label: "CPU", value: 40, min: 0, max: 100, unit: "%", color: "accent" },
  donut: { kind: "donut", slices: [{ label: "x", value: 3 }, { label: "y", value: 1, color: "series3" }], label: "Total", value: null },
  heatmap: { kind: "heatmap", values: [[0, 1, 2], [3, 4, 0]], rows: ["a", "b"], columns: ["1", "2", "3"], color: "accent" },
  progress: { kind: "progress", label: "Build", value: 30, max: 60, detail: "half", color: "positive" },
  canvas: {
    kind: "canvas",
    width: 100,
    height: 50,
    shapes: [
      { kind: "rect", x: 1, y: 1, width: 10, height: 10, color: "accent", radius: 0, stroke: null, stroke_width: 1.5, fill: true },
      { kind: "circle", x: 20, y: 20, radius: 5, color: "positive", stroke: null, stroke_width: 1.5, fill: true },
      { kind: "line", x1: 0, y1: 0, x2: 9, y2: 9, width: 1, color: "ink", arrow: "none", dashed: false },
      { kind: "polyline", points: [[0, 0], [5, 5], [9, 2]], width: 1, color: "warning", arrow: "none", dashed: false },
      { kind: "label", x: 4, y: 4, text: "hi", color: "ink", align: "start", baseline: "top", chip: false },
    ],
  },
};

describe("visual tree (F-73)", () => {
  it.each(Object.entries(KINDS))("draws the %s node as inline SVG", (_name, node) => {
    const { container } = render(<VisualTree node={node} />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("viewBox")).toMatch(/^0 0 \d+ \d+$/);
    // The halftone dither is painted as filled paths, one per colour.
    if (["bar", "area", "gauge", "donut", "heatmap", "progress"].includes(_name)) expect(container.querySelectorAll("svg path").length).toBeGreaterThan(0);
  });

  it("draws a stat with its trend, a table and the layout nodes", () => {
    const tree: VisualNode = {
      kind: "column",
      gap: 8,
      children: [
        { kind: "text", text: "Weekly report", size: "title", color: "ink" },
        { kind: "row", gap: 8, children: [{ kind: "stat", label: "Revenue", value: "$12k", detail: "+12.4% vs last month", color: "accent", trend: [1, 2, 4, 3] }] },
        { kind: "grid", columns: 2, gap: 8, children: [{ kind: "table", columns: ["Name", "Qty"], rows: [["Widget", "4"]] }] },
        { kind: "card", gap: 4, children: [{ kind: "progress", label: "Goal", value: 1, max: 2, color: "accent" }] },
      ],
    };
    const { container } = render(<VisualTree node={tree} />);
    expect(screen.getByText("Weekly report")).toBeInTheDocument();
    expect(screen.getByText("$12k")).toBeInTheDocument();
    expect(screen.getByText(/\+12\.4%/)).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Qty" })).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "Widget" })).toBeInTheDocument();
    // The stat's trend and the progress bar are SVG.
    expect(container.querySelectorAll("svg").length).toBeGreaterThanOrEqual(2);
  });

  const controls: VisualNode = {
    kind: "column",
    gap: 4,
    children: [
      { kind: "button", label: "Run", handler: "1:0", disabled: false },
      { kind: "toggle", label: "Auto", value: true, handler: "1:1" },
      { kind: "input", label: "Name", value: "x", handler: "1:2" },
      { kind: "select", label: "Mode", options: ["a", "b"], selected: 1, handler: "1:3" },
      { kind: "slider", label: "Level", value: 5, min: 0, max: 10, step: 1, handler: "1:4" },
      { kind: "tabs", labels: ["One", "Two"], selected: 0, handler: "1:5" },
    ],
  };

  it("draws every control disabled, with one hint for the visual, when nothing can act on it", () => {
    const { container } = render(<VisualTree node={controls} />);
    expect(screen.getByText("Run")).toBeInTheDocument();
    const live = [...container.querySelectorAll("button, input, select")];
    expect(live).toHaveLength(7);
    expect(live.every((control) => (control as HTMLButtonElement).disabled)).toBe(true);
    expect(screen.getAllByText("Open on desktop to interact")).toHaveLength(1);
    // A visual without controls has nothing to say.
    render(<VisualTree node={{ kind: "text", text: "plain", size: "body", color: "ink" }} />);
    expect(screen.getAllByText("Open on desktop to interact")).toHaveLength(1);
  });

  it("reports each control with the handler and the value it takes", async () => {
    const user = userEvent.setup();
    const onDispatch = vi.fn();
    render(<VisualTree node={controls} onDispatch={onDispatch} />);
    expect(screen.queryByText("Open on desktop to interact")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Run" }));
    expect(onDispatch).toHaveBeenLastCalledWith("1:0", null);
    const toggle = screen.getByRole("switch", { name: "Auto" });
    expect(toggle).toHaveAttribute("aria-checked", "true");
    await user.click(toggle);
    expect(onDispatch).toHaveBeenLastCalledWith("1:1", false);
    await user.selectOptions(screen.getByRole("combobox", { name: "Mode" }), "0");
    expect(onDispatch).toHaveBeenLastCalledWith("1:3", 0);
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((tab) => tab.getAttribute("aria-selected"))).toEqual(["true", "false"]);
    await user.click(screen.getByRole("tab", { name: "Two" }));
    expect(onDispatch).toHaveBeenLastCalledWith("1:5", 1);
    // The tab that is already selected sends nothing.
    onDispatch.mockClear();
    await user.click(screen.getByRole("tab", { name: "One" }));
    expect(onDispatch).not.toHaveBeenCalled();
  });

  it("sends an input on Enter or blur and not per keystroke, and never more than the desktop accepts", async () => {
    const user = userEvent.setup();
    const onDispatch = vi.fn();
    const { rerender } = render(<VisualTree node={controls} onDispatch={onDispatch} />);
    const input = screen.getByRole("textbox", { name: "Name" });
    await user.type(input, "yz");
    expect(onDispatch).not.toHaveBeenCalled();
    await user.keyboard("{Enter}");
    expect(onDispatch).toHaveBeenCalledTimes(1);
    expect(onDispatch).toHaveBeenLastCalledWith("1:2", "xyz");
    await user.type(input, "!");
    await user.tab();
    expect(onDispatch).toHaveBeenLastCalledWith("1:2", "xyz!");
    // The desktop's next tree holds the text: a blur that changed nothing sends nothing.
    const next = structuredClone(controls) as Extract<VisualNode, { children: VisualNode[] }>;
    next.children[2] = { kind: "input", label: "Name", value: "xyz!", handler: "2:2" };
    rerender(<VisualTree node={next} onDispatch={onDispatch} />);
    onDispatch.mockClear();
    await user.click(input);
    await user.tab();
    expect(onDispatch).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: "é".repeat(2049) } });
    expect(input).toHaveValue("xyz!");
  });

  it("sends a slider at once, throttled while it is dragged, and the last value when it is released", () => {
    vi.useFakeTimers();
    try {
      const onDispatch = vi.fn();
      render(<VisualTree node={controls} onDispatch={onDispatch} />);
      const slider = screen.getByRole("slider", { name: "Level" });
      fireEvent.change(slider, { target: { value: "6" } });
      expect(onDispatch).toHaveBeenCalledTimes(1);
      expect(onDispatch).toHaveBeenLastCalledWith("1:4", 6);
      fireEvent.change(slider, { target: { value: "7" } });
      fireEvent.change(slider, { target: { value: "8" } });
      expect(onDispatch).toHaveBeenCalledTimes(1);
      // Released before the throttle ended: the last value goes now.
      fireEvent.pointerUp(slider);
      expect(onDispatch).toHaveBeenCalledTimes(2);
      expect(onDispatch).toHaveBeenLastCalledWith("1:4", 8);
      expect(screen.getByText("8")).toBeInTheDocument();
      // Nothing is left over to send later.
      vi.advanceTimersByTime(1000);
      expect(onDispatch).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("does not let a reply overwrite what is being typed or dragged", async () => {
    const user = userEvent.setup();
    const onDispatch = vi.fn();
    const withValues = (level: number, name: string, tag: string): VisualNode => {
      const next = structuredClone(controls) as Extract<VisualNode, { children: VisualNode[] }>;
      next.children[2] = { kind: "input", label: "Name", value: name, handler: `${tag}:2` };
      next.children[4] = { kind: "slider", label: "Level", value: level, min: 0, max: 10, step: 1, handler: `${tag}:4` };
      return next;
    };
    const { rerender } = render(<VisualTree node={controls} onDispatch={onDispatch} />);
    const input = screen.getByRole("textbox", { name: "Name" });
    await user.type(input, "yz");
    // A reply of an earlier send arrives while the text is not sent: the text stays.
    rerender(<VisualTree node={withValues(5, "x", "2")} onDispatch={onDispatch} />);
    expect(input).toHaveValue("xyz");
    await user.keyboard("{Enter}");
    // After the send, the desktop's value takes over.
    rerender(<VisualTree node={withValues(5, "xyz", "3")} onDispatch={onDispatch} />);
    expect(input).toHaveValue("xyz");
    rerender(<VisualTree node={withValues(5, "snapped", "4")} onDispatch={onDispatch} />);
    expect(input).toHaveValue("snapped");

    const slider = screen.getByRole("slider", { name: "Level" });
    fireEvent.pointerDown(slider);
    fireEvent.change(slider, { target: { value: "8" } });
    rerender(<VisualTree node={withValues(3, "snapped", "5")} onDispatch={onDispatch} />);
    expect(screen.getByText("8")).toBeInTheDocument();
    fireEvent.pointerUp(slider);
    rerender(<VisualTree node={withValues(3, "snapped", "6")} onDispatch={onDispatch} />);
    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("moves between tabs with the arrow keys, Home and End, and only the selected tab is in the tab order", async () => {
    const user = userEvent.setup();
    const onDispatch = vi.fn();
    const tabs: VisualNode = { kind: "tabs", labels: ["One", "Two", "Three"], selected: 1, handler: "1:0" };
    render(<VisualTree node={tabs} onDispatch={onDispatch} />);
    expect(screen.getAllByRole("tab").map((tab) => tab.getAttribute("tabindex"))).toEqual(["-1", "0", "-1"]);
    screen.getByRole("tab", { name: "Two" }).focus();
    await user.keyboard("{ArrowRight}");
    expect(onDispatch).toHaveBeenLastCalledWith("1:0", 2);
    expect(screen.getByRole("tab", { name: "Three" })).toHaveFocus();
    await user.keyboard("{Home}");
    expect(onDispatch).toHaveBeenLastCalledWith("1:0", 0);
    await user.keyboard("{End}");
    expect(onDispatch).toHaveBeenLastCalledWith("1:0", 2);
    await user.keyboard("{ArrowLeft}");
    expect(onDispatch).toHaveBeenLastCalledWith("1:0", 0);
  });

  it("keeps a disabled button off and shows a pending state and an error", () => {
    const onDispatch = vi.fn();
    render(<VisualTree node={{ kind: "button", label: "Off", handler: "1:0", disabled: true }} onDispatch={onDispatch} pending error="Too many taps" />);
    expect(screen.getByRole("button", { name: "Off" })).toBeDisabled();
    expect(screen.getByTestId("visual-tree")).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("alert")).toHaveTextContent("Too many taps");
  });

  it("renders hostile text as text, and the sources contain no HTML sink", () => {
    const hostile = '<img src=x onerror="alert(1)"><script>alert(1)</script>';
    const { container } = render(<VisualTree node={{ kind: "text", text: hostile, size: "body", color: "ink" }} />);
    expect(container.querySelector("img, script")).toBeNull();
    expect(container.textContent).toContain(hostile);
    for (const file of readdirSync(__dirname).filter((name) => /\.tsx?$/.test(name) && !name.endsWith(".test.tsx"))) {
      expect(readFileSync(join(__dirname, file), "utf8"), file).not.toMatch(/dangerouslySetInnerHTML|innerHTML|insertAdjacentHTML|DOMParser/);
    }
  });

  it("draws nothing for an empty chart and a 3D scene as a notice", () => {
    const empty = render(<VisualTree node={{ kind: "chart", chart_type: "bar", series: [], labels: [], height: 100 }} />);
    expect(empty.container.querySelector("svg")).toBeNull();
    empty.unmount();
    render(<VisualTree node={{ kind: "scene", width: 4, height: 3 }} />);
    expect(screen.getByText(/3D scene on your desktop/)).toBeInTheDocument();
  });
});

/** How many of the 16 cells of one Bayer tile are inked at `tone`. */
const dither = (tone: number) => Array.from({ length: 16 }, (_, i) => inked(i % 4, Math.floor(i / 4), tone)).filter(Boolean).length;

describe("dither", () => {
  it("inks a share of cells that matches the tone", () => {
    expect(dither(0.25)).toBe(4);
    expect(dither(0)).toBe(0);
    expect(dither(1)).toBe(16);
  });
});

describe("contribution graph (F-74)", () => {
  it("draws 26 weeks as a grid with weekday and month labels", () => {
    const levels = Array.from({ length: 26 * 7 }, (_, i) => i % 5);
    const { container } = render(<ContributionGraph levels={levels} today={new Date(2026, 10, 18)} />);
    const svg = container.querySelector("svg") as SVGSVGElement;
    // 26 weeks of 5 cells of 3 units, after the 28 unit gutter.
    expect(svg.getAttribute("viewBox")).toBe(`0 0 ${28 + 26 * 15} ${18 + 7 * 15}`);
    expect(["Mon", "Wed", "Fri"].every((day) => screen.getByText(day))).toBe(true);
    expect(container.querySelectorAll("text").length).toBeGreaterThan(5);
    expect(container.querySelectorAll("path").length).toBeGreaterThan(0);
  });
});

describe("parseVisualTree (data from the desktop is checked)", () => {
  const text = { kind: "text", text: "hi", size: "body", color: "ink" };

  it("reads a well-formed tree and nothing else", () => {
    expect(parseVisualTree({ kind: "column", gap: 4, children: [text] })).toEqual({ kind: "column", gap: 4, children: [text] });
    for (const bad of [null, undefined, 3, "x", [], {}, { kind: 4 }]) expect(parseVisualTree(bad)).toBeNull();
  });

  it("refuses a node with a missing or wrongly typed field", () => {
    expect(parseVisualTree({ kind: "column", gap: 4 })).toBeNull();
    expect(parseVisualTree({ kind: "column", gap: 4, children: "x" })).toBeNull();
    expect(parseVisualTree({ kind: "column", gap: 4, children: [{ kind: "text", size: "body" }] })).toBeNull();
    expect(parseVisualTree({ kind: "table", columns: null, rows: [] })).toBeNull();
    expect(parseVisualTree({ kind: "gauge", label: "x", value: "40", min: 0, max: 1 })).toBeNull();
    expect(parseVisualTree({ kind: "chart", chart_type: "pie", series: [], labels: [], height: 200 })).toBeNull();
    expect(parseVisualTree({ kind: "chart", chart_type: "bar", series: [{ label: "a", values: [1, Number.NaN] }], labels: [], height: 200 })).toBeNull();
  });

  it("skips a node kind it does not know, as the renderer always did", () => {
    expect(parseVisualTree({ kind: "column", gap: 4, children: [{ kind: "hologram" }, text] })).toEqual({ kind: "column", gap: 4, children: [text] });
    expect(parseVisualTree({ kind: "hologram" })).toBeNull();
  });

  it("clamps a chart height to the desktop's 120 to 900", () => {
    const chart = (height: number) => parseVisualTree({ kind: "chart", chart_type: "line", series: [{ label: "a", values: [1, 2] }], labels: ["x", "y"], height });
    expect(chart(5)).toMatchObject({ height: CHART_HEIGHT.min });
    expect(chart(5000)).toMatchObject({ height: CHART_HEIGHT.max });
    expect(chart(300)).toMatchObject({ height: 300 });
  });

  it("refuses more than 512 nodes, more than 16 levels and oversized series", () => {
    expect(parseVisualTree({ kind: "column", gap: 0, children: Array.from({ length: MAX_NODES }, () => text) })).toBeNull();
    expect(parseVisualTree({ kind: "column", gap: 0, children: Array.from({ length: MAX_NODES - 1 }, () => text) })).not.toBeNull();
    let deep: unknown = text;
    for (let i = 0; i < MAX_DEPTH; i += 1) deep = { kind: "column", gap: 0, children: [deep] };
    expect(parseVisualTree(deep)).toBeNull();
    expect(parseVisualTree({ kind: "chart", chart_type: "bar", series: [{ label: "a", values: new Array(513).fill(1) }], labels: [], height: 200 })).toBeNull();
  });

  it("falls back to a line of text when a node still cannot be drawn", () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    const broken = { kind: "table", columns: null, rows: [] } as unknown as VisualNode;
    render(<VisualTree node={broken} />);
    expect(screen.getByText(/This visual could not be drawn/)).toBeInTheDocument();
    quiet.mockRestore();
  });

  it("draws the last 26 weeks of a longer contribution list", () => {
    const { container } = render(<ContributionGraph levels={Array.from({ length: 400 }, (_, i) => i % 5)} today={new Date(2026, 10, 18)} />);
    expect(container.querySelector("svg")?.getAttribute("viewBox")).toBe(`0 0 ${28 + 26 * 15} ${18 + 7 * 15}`);
  });
});
