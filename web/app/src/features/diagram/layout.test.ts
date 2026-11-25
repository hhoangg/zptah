import { describe, expect, it } from "vitest";
import { fixtureNames, readGolden, readGraph, readMermaid, readParsed } from "./fixtures";
import { type Layout, arrowhead, bottom, intersects, layout, right, smooth, textWidth, wrapLabel } from "./layout";
import { parseFlowchart } from "./mermaid";
import type { Direction, FlowEdge, FlowGraph, FlowNode } from "./model";
import { MAX_FLOW_NODES, validateGraph } from "./model";

const node = (id: string, label = `Node ${id}`): FlowNode => ({ id, label, kind: "box", color: null });
const edge = (from: string, to: string, label: string | null = null): FlowEdge => ({ from, to, label, dashed: false, arrow: true });
const graph = (direction: Direction, ids: string[], edges: [string, string, (string | null)?][]): FlowGraph => ({
  direction,
  nodes: ids.map((id) => node(id)),
  edges: edges.map(([a, b, l]) => edge(a, b, l ?? null)),
});

/** No two nodes overlap, no label chip touches a node or another chip, and everything stays inside the reported size. */
function assertClean(g: FlowGraph): Layout {
  const l = layout(g);
  l.nodes.forEach((a, i) => {
    expect(a.rect.x).toBeGreaterThanOrEqual(0);
    expect(a.rect.y).toBeGreaterThanOrEqual(0);
    expect(right(a.rect)).toBeLessThanOrEqual(l.width);
    expect(bottom(a.rect)).toBeLessThanOrEqual(l.height);
    for (const b of l.nodes.slice(i + 1)) expect(intersects(a.rect, b.rect), `nodes ${g.nodes[a.index]!.id} and ${g.nodes[b.index]!.id} overlap`).toBe(false);
  });
  const labels = l.edges.flatMap((e) => (e.label ? [e.label] : []));
  labels.forEach((a, i) => {
    expect(right(a.rect)).toBeLessThanOrEqual(l.width);
    expect(bottom(a.rect)).toBeLessThanOrEqual(l.height);
    for (const n of l.nodes) expect(intersects(a.rect, n.rect), `label '${a.text}' covers node ${g.nodes[n.index]!.id}`).toBe(false);
    for (const b of labels.slice(i + 1)) expect(intersects(a.rect, b.rect), `labels '${a.text}' and '${b.text}' overlap`).toBe(false);
  });
  return l;
}

const chain = (n: number): FlowGraph => {
  const ids = Array.from({ length: n }, (_, i) => `n${i}`);
  return graph("LR", ids, ids.slice(1).map((id, i) => [ids[i]!, id]));
};

describe("flow layout", () => {
  it("is deterministic", () => {
    const g = graph("TB", ["a", "b", "c", "d", "e"], [["a", "b", "x"], ["a", "c"], ["b", "d"], ["c", "d", "y"], ["d", "a"], ["a", "e"]]);
    const first = layout(g);
    for (let i = 0; i < 5; i++) expect(layout(structuredClone(g))).toEqual(first);
  });

  it("puts layers along the flow in every direction", () => {
    const g = graph("LR", ["a", "b", "c"], [["a", "b"], ["b", "c"]]);
    let l = assertClean(g);
    expect(right(l.nodes[0]!.rect)).toBeLessThan(l.nodes[1]!.rect.x);
    expect(right(l.nodes[1]!.rect)).toBeLessThan(l.nodes[2]!.rect.x);
    l = assertClean({ ...g, direction: "TB" });
    expect(bottom(l.nodes[0]!.rect)).toBeLessThan(l.nodes[1]!.rect.y);
    l = assertClean({ ...g, direction: "RL" });
    expect(right(l.nodes[2]!.rect)).toBeLessThan(l.nodes[0]!.rect.x);
    l = assertClean({ ...g, direction: "BT" });
    expect(bottom(l.nodes[2]!.rect)).toBeLessThan(l.nodes[0]!.rect.y);
  });

  it("has no overlaps for a diamond with edge labels, in both axes", () => {
    const g = graph("LR", ["a", "b", "c", "d", "e", "f"], [["a", "b"], ["b", "c", "safe"], ["b", "d", "risky"], ["d", "e", "allow"], ["d", "f", "deny"], ["c", "e", "skip a long layer"]]);
    assertClean(g);
    assertClean({ ...g, direction: "TB" });
  });

  it("lays out a cycle and a self loop, the back edge still ending on its original target", () => {
    const g = graph("LR", ["a", "b", "c"], [["a", "b", "go"], ["b", "c"], ["c", "a", "retry"], ["b", "b", "again"]]);
    const l = assertClean(g);
    const end = l.edges[2]!.points.at(-1)!;
    const a = l.nodes[0]!.rect;
    expect(Math.abs(end[0] - right(a)) < 1 || Math.abs(end[0] - a.x) < 1).toBe(true);
    expect(end[1]).toBeGreaterThanOrEqual(a.y);
    expect(end[1]).toBeLessThanOrEqual(bottom(a));
    const own = l.edges[3]!.points;
    const b = l.nodes[1]!.rect;
    expect(own.length).toBeGreaterThanOrEqual(3);
    expect(own.every((p) => p[1] >= b.y && p[1] <= bottom(b))).toBe(true);
    assertClean({ ...g, direction: "TB" });
  });

  it("lays out 30 nodes with cross links, a cycle and a self loop without overlaps", () => {
    const ids = Array.from({ length: 30 }, (_, i) => `n${i}`);
    const edges: [string, string, (string | null)?][] = [];
    for (let i = 1; i < 30; i++) edges.push([ids[Math.floor((i - 1) / 2)]!, ids[i]!, i % 3 === 0 ? "yes" : null]);
    edges.push([ids[29]!, ids[3]!, "loop back"], [ids[5]!, ids[28]!], [ids[0]!, ids[20]!, "long"], [ids[7]!, ids[7]!]);
    const g = graph("LR", ids, edges);
    expect(assertClean(g).nodes).toHaveLength(30);
    assertClean({ ...g, direction: "TB" });
  });

  it("handles a long chain and disconnected nodes, wrapping a long label", () => {
    assertClean(chain(30));
    const g = chain(4);
    g.nodes.push(node("lonely", "Alone with a very long label that must wrap onto several lines"), node("other", "Other"));
    expect(assertClean(g).nodes[4]!.lines.length).toBeGreaterThanOrEqual(3);
  });

  it("keeps a long edge away from the nodes it skips", () => {
    const l = assertClean(graph("LR", ["a", "b", "c", "d"], [["a", "b"], ["b", "c"], ["c", "d"], ["a", "d", "skip"]]));
    const skip = l.edges[3]!.points;
    for (let i = 0; i + 1 < skip.length; i++) {
      const [p, q] = [skip[i]!, skip[i + 1]!];
      const bounds = { x: Math.min(p[0], q[0]), y: Math.min(p[1], q[1]), w: Math.max(Math.abs(p[0] - q[0]), 0.01), h: Math.max(Math.abs(p[1] - q[1]), 0.01) };
      for (const index of [1, 2]) expect(intersects(bounds, l.nodes[index]!.rect), `skip edge crosses node ${index}`).toBe(false);
    }
  });

  it("routes orthogonally and ends on the target side", () => {
    const l = assertClean(graph("LR", ["a", "b", "c"], [["a", "b"], ["a", "c"], ["b", "c"]]));
    for (const e of l.edges) {
      for (let i = 0; i + 1 < e.points.length; i++) {
        const [p, q] = [e.points[i]!, e.points[i + 1]!];
        expect(Math.abs(p[0] - q[0]) < 0.01 || Math.abs(p[1] - q[1]) < 0.01, `diagonal ${p} ${q}`).toBe(true);
      }
      const tip = e.points.at(-1)!;
      const target = l.nodes[e.index === 0 ? 1 : 2]!.rect;
      expect(tip[1]).toBeGreaterThanOrEqual(target.y - 0.5);
      expect(tip[1]).toBeLessThanOrEqual(bottom(target) + 0.5);
      expect(Math.abs(tip[0] - target.x)).toBeLessThan(0.5);
    }
  });

  it("reports clear validation errors", () => {
    const g = graph("LR", ["a", "b"], [["a", "zzz"]]);
    expect(validateGraph(g)).toMatch(/unknown node id 'zzz' in to/);
    g.edges[0] = edge("nope", "a");
    expect(validateGraph(g)).toContain("'nope'");
    g.edges = [];
    g.nodes.push(node("a", "again"));
    expect(validateGraph(g)).toContain("Duplicate flow node id 'a'");
    expect(() => layout(g)).toThrow();
    expect(validateGraph({ direction: "LR", nodes: [], edges: [] })).not.toBeNull();
    const many = chain(MAX_FLOW_NODES + 1);
    expect(validateGraph(many)).not.toBeNull();
    many.nodes.length = MAX_FLOW_NODES;
    many.edges.length = MAX_FLOW_NODES - 1;
    expect(validateGraph(many)).toBeNull();
  });

  it("has text and geometry helpers that agree with the desktop", () => {
    const head = arrowhead([10, 0], [0, 0], 8);
    expect(head[0]).toEqual([10, 0]);
    expect(head[1][0]).toBeCloseTo(2, 4);
    expect(head[2][0]).toBeCloseTo(2, 4);
    expect(head[1][1] * head[2][1]).toBeLessThan(0);
    const smoothed = smooth([[0, 0], [10, 0], [10, 10]], 4);
    expect(smoothed[0]).toEqual([0, 0]);
    expect(smoothed.at(-1)).toEqual([10, 10]);
    expect(smoothed.length).toBeGreaterThan(3);
    expect(smooth([[0, 0], [1, 1]], 4)).toHaveLength(2);
    expect(wrapLabel("a b")).toHaveLength(1);
    expect(wrapLabel("one\ntwo")).toHaveLength(2);
    expect(wrapLabel("x".repeat(60)).length).toBeGreaterThanOrEqual(3);
    expect(textWidth("abcd", 10)).toBeCloseTo(24, 4);
    expect(textWidth("中", 10)).toBeCloseTo(12, 4);
  });
});

const close = (a: number, b: number) => Math.abs(a - b) < 0.02;
function expectMatchesRust(laid: Layout, golden: NonNullable<ReturnType<typeof readGolden>>) {
  expect(close(laid.width, golden.width), `width ${laid.width} vs ${golden.width}`).toBe(true);
  expect(close(laid.height, golden.height), `height ${laid.height} vs ${golden.height}`).toBe(true);
  expect(laid.nodes.map((n) => n.lines)).toEqual(golden.nodes.map((n) => n.lines));
  laid.nodes.forEach((n, i) => {
    const g = golden.nodes[i]!.rect;
    expect([n.rect.x, n.rect.y, n.rect.w, n.rect.h].every((v, k) => close(v, g[k]!)), `node ${i} ${JSON.stringify(n.rect)} vs ${g}`).toBe(true);
  });
  laid.edges.forEach((e, i) => {
    const g = golden.edges[i]!;
    expect(e.points.length, `edge ${i} point count`).toBe(g.points.length);
    e.points.forEach((p, k) => expect(close(p[0], g.points[k]![0]) && close(p[1], g.points[k]![1]), `edge ${i} point ${k}: ${p} vs ${g.points[k]}`).toBe(true));
    expect(e.label?.text).toBe(g.label?.text);
    if (e.label && g.label) expect([e.label.rect.x, e.label.rect.y, e.label.rect.w, e.label.rect.h].every((v, k) => close(v, g.label!.rect[k]!)), `edge ${i} label`).toBe(true);
  });
}

describe("parity with the Rust layout (shared fixtures)", () => {
  const names = fixtureNames(".graph.json");
  it("has fixtures", () => expect(names.length).toBeGreaterThanOrEqual(5));
  it.each(names)("lays out %s like the desktop", (name) => {
    const golden = readGolden(name);
    expect(golden, `fixtures/${name}.layout.json (regenerate with ZPTAH_UPDATE_DIAGRAM_GOLDENS=1, see crates/monocode-core/src/diagram/golden.rs)`).not.toBeNull();
    expectMatchesRust(layout(readGraph(name)), golden!);
    assertClean(readGraph(name));
  });
  it.each(fixtureNames(".mmd"))("parses and lays out mermaid %s like the desktop", (name) => {
    const rust = readParsed(name);
    expect(rust, `fixtures/${name}.parsed.json`).not.toBeNull();
    const parsed = parseFlowchart(readMermaid(name));
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.graph).toEqual(rust!.graph);
    expectMatchesRust(layout(parsed.graph), rust!.layout);
    assertClean(parsed.graph);
  });
});
