// Deterministic layered layout for flow diagrams, a step by step port of crates/monocode-core/src/diagram/layout.rs
// (read the algorithm notes at the top of that file; keep the steps and constants in sync). Pure functions only.
// Numbers are doubles here and single precision floats there, so the results match to within rounding.
import { type Direction, type FlowGraph, type NodeKind, isHorizontal, validateGraph } from "./model";

export const NODE_FONT = 14;
export const EDGE_FONT = 12;
export const LINE_HEIGHT = 20;
const WRAP_COLUMNS = 26;
const PAD = 24;
const RANK_GAP = 60;
const NODE_GAP = 36;
const DUMMY_GAP = 14;
const LANE_SPACING = 10;
const LOOP = 22;
const CHIP_H = 22;
const CHIP_PAD = 14;
const CHIP_MARGIN = 10;

export type Rect = { x: number; y: number; w: number; h: number };
export type Point = [number, number];
export type PlacedNode = { index: number; rect: Rect; lines: string[] };
export type PlacedLabel = { rect: Rect; text: string };
export type PlacedEdge = { index: number; points: Point[]; label: PlacedLabel | null };
export type Layout = { width: number; height: number; nodes: PlacedNode[]; edges: PlacedEdge[] };

export const right = (r: Rect) => r.x + r.w;
export const bottom = (r: Rect) => r.y + r.h;
/** Strict overlap: rectangles that only touch do not intersect. */
export const intersects = (a: Rect, b: Rect) => a.x < right(b) && b.x < right(a) && a.y < bottom(b) && b.y < bottom(a);

function charCells(c: number): number {
  if (
    (c >= 0x1100 && c <= 0x115f) ||
    (c >= 0x2e80 && c <= 0xa4cf) ||
    (c >= 0xac00 && c <= 0xd7a3) ||
    (c >= 0xf900 && c <= 0xfaff) ||
    (c >= 0xfe30 && c <= 0xfe6f) ||
    (c >= 0xff00 && c <= 0xff60) ||
    (c >= 0xffe0 && c <= 0xffe6) ||
    (c >= 0x1f300 && c <= 0x1faff)
  ) {
    return 2;
  }
  return 1;
}

const cellsOf = (text: string): number => {
  let cells = 0;
  for (const ch of text) cells += charCells(ch.codePointAt(0) ?? 0);
  return cells;
};

/** Approximate advance of `text` in the monospace font used by diagrams. */
export const textWidth = (text: string, font: number): number => cellsOf(text) * 0.6 * font;

/** Wrap a label into lines of at most 26 columns; an explicit newline always breaks. Words longer than a line are split. */
export function wrapLabel(label: string): string[] {
  const lines: string[] = [];
  for (const paragraph of label.split("\n")) {
    let line = "";
    let cells = 0;
    for (const word of paragraph.split(/\s+/).filter(Boolean)) {
      const wordCells = cellsOf(word);
      if (line !== "" && cells + 1 + wordCells > WRAP_COLUMNS) {
        lines.push(line);
        line = "";
        cells = 0;
      }
      if (wordCells > WRAP_COLUMNS) {
        for (const c of word) {
          const w = charCells(c.codePointAt(0) ?? 0);
          if (cells + w > WRAP_COLUMNS) {
            lines.push(line);
            line = "";
            cells = 0;
          }
          line += c;
          cells += w;
        }
        continue;
      }
      if (line !== "") {
        line += " ";
        cells += 1;
      }
      line += word;
      cells += wordCells;
    }
    lines.push(line);
  }
  return lines;
}

type Sized = { w: number; h: number; lines: string[] };

function nodeSize(label: string, kind: NodeKind): Sized {
  const lines = wrapLabel(label);
  const tw = lines.reduce((max, l) => Math.max(max, textWidth(l, NODE_FONT)), 0);
  const th = lines.length * LINE_HEIGHT;
  switch (kind) {
    case "box":
    case "round":
      return { w: Math.max(tw + 32, 72), h: Math.max(th + 20, 40), lines };
    case "start":
    case "end":
      return { w: Math.max(tw + 40, 80), h: Math.max(th + 20, 40), lines };
    case "decision":
      return { w: Math.max(1.7 * tw + 40, 96), h: Math.max(2 * th + 16, 56), lines };
    case "circle": {
      const d = Math.max(tw + 28, th + 28, 56);
      return { w: d, h: d, lines };
    }
  }
}

type Item = { real: number | null; layer: number; main: number; cross: number; pos: number; m0: number };
type Link = { u: number; v: number };
type Chain = { edge: number; reversed: boolean; links: number[] };
type Chip = { main: number; cross: number; text: string };
/** [main, cross, size main, size cross] */
type Box = [number, number, number, number];

function chipOf(label: string, horizontal: boolean): Chip {
  const w = textWidth(label, EDGE_FONT) + CHIP_PAD;
  return horizontal ? { main: w, cross: CHIP_H, text: label } : { main: CHIP_H, cross: w, text: label };
}

const byNumber = (a: number, b: number) => (a < b ? -1 : a > b ? 1 : 0);

/** Drop repeated points and merge collinear runs. */
function dedupe(points: Point[]): Point[] {
  const out: Point[] = [];
  for (const p of points) {
    const last = out[out.length - 1];
    if (last && Math.abs(last[0] - p[0]) < 0.01 && Math.abs(last[1] - p[1]) < 0.01) continue;
    while (out.length >= 2) {
      const a = out[out.length - 2]!;
      const b = out[out.length - 1]!;
      const sameX = Math.abs(a[0] - b[0]) < 0.01 && Math.abs(b[0] - p[0]) < 0.01;
      const sameY = Math.abs(a[1] - b[1]) < 0.01 && Math.abs(b[1] - p[1]) < 0.01;
      if (sameX || sameY) out.pop();
      else break;
    }
    out.push(p);
  }
  return out;
}

export function layout(graph: FlowGraph): Layout {
  const invalid = validateGraph(graph);
  if (invalid) throw new Error(invalid);
  const horizontal = isHorizontal(graph.direction);
  const n = graph.nodes.length;
  const index = new Map(graph.nodes.map((node, i) => [node.id, i] as const));
  const ends: [number, number][] = graph.edges.map((e) => [index.get(e.from)!, index.get(e.to)!]);
  const sized = graph.nodes.map((node) => nodeSize(node.label, node.kind));

  // 2. Break cycles.
  const adjacency: [number, number][][] = Array.from({ length: n }, () => []);
  ends.forEach(([a, b], e) => adjacency[a]!.push([e, b]));
  const state = new Array<number>(n).fill(0);
  const reversed = new Array<boolean>(ends.length).fill(false);
  for (let start = 0; start < n; start++) {
    if (state[start] !== 0) continue;
    state[start] = 1;
    const stack: [number, number][] = [[start, 0]];
    while (stack.length > 0) {
      const top = stack[stack.length - 1]!;
      const [u, i] = top;
      if (i < adjacency[u]!.length) {
        top[1] += 1;
        const [e, v] = adjacency[u]![i]!;
        if (v === u) continue;
        if (state[v] === 0) {
          state[v] = 1;
          stack.push([v, 0]);
        } else if (state[v] === 1) {
          reversed[e] = true;
        }
      } else {
        state[u] = 2;
        stack.pop();
      }
    }
  }
  const oriented: ([number, number] | null)[] = ends.map(([a, b], e) => (a === b ? null : reversed[e] ? [b, a] : [a, b]));

  // 3. Longest-path layering.
  const indegree = new Array<number>(n).fill(0);
  const forward: number[][] = Array.from({ length: n }, () => []);
  for (const pair of oriented) {
    if (!pair) continue;
    indegree[pair[1]]! += 1;
    forward[pair[0]]!.push(pair[1]);
  }
  const layerOf = new Array<number>(n).fill(0);
  const queue: number[] = [];
  for (let i = 0; i < n; i++) if (indegree[i] === 0) queue.push(i);
  for (let head = 0; head < queue.length; head++) {
    const u = queue[head]!;
    for (const v of forward[u]!) {
      layerOf[v] = Math.max(layerOf[v]!, layerOf[u]! + 1);
      indegree[v]! -= 1;
      if (indegree[v] === 0) queue.push(v);
    }
  }
  const layerCount = Math.max(...layerOf, 0) + 1;

  // 4. Items, chains and links.
  const items: Item[] = sized.map((s, i) => ({
    real: i,
    layer: layerOf[i]!,
    main: horizontal ? s.w : s.h,
    cross: horizontal ? s.h : s.w,
    pos: 0,
    m0: 0,
  }));
  const links: Link[] = [];
  const chains: Chain[] = [];
  oriented.forEach((pair, e) => {
    if (!pair) return;
    const [a, b] = pair;
    const chainLinks: number[] = [];
    let prev = a;
    for (let layer = layerOf[a]! + 1; layer < layerOf[b]!; layer++) {
      items.push({ real: null, layer, main: 0, cross: 0, pos: 0, m0: 0 });
      const dummy = items.length - 1;
      chainLinks.push(links.length);
      links.push({ u: prev, v: dummy });
      prev = dummy;
    }
    chainLinks.push(links.length);
    links.push({ u: prev, v: b });
    chains.push({ edge: e, reversed: reversed[e]!, links: chainLinks });
  });
  const count = items.length;
  const up: number[][] = Array.from({ length: count }, () => []);
  const down: number[][] = Array.from({ length: count }, () => []);
  for (const link of links) {
    down[link.u]!.push(link.v);
    up[link.v]!.push(link.u);
  }

  // 5. Order inside layers.
  const rank = new Array<number>(count).fill(Number.MAX_SAFE_INTEGER);
  let visited = 0;
  for (let root = 0; root < count; root++) {
    if (up[root]!.length > 0 || rank[root] !== Number.MAX_SAFE_INTEGER) continue;
    const stack = [root];
    while (stack.length > 0) {
      const u = stack.pop()!;
      if (rank[u] !== Number.MAX_SAFE_INTEGER) continue;
      rank[u] = visited;
      visited += 1;
      for (let k = down[u]!.length - 1; k >= 0; k--) stack.push(down[u]![k]!);
    }
  }
  let layers: number[][] = Array.from({ length: layerCount }, () => []);
  items.forEach((item, i) => layers[item.layer]!.push(i));
  for (const layer of layers) layer.sort((a, b) => byNumber(rank[a]!, rank[b]!) || byNumber(a, b));
  const crossings = (ls: number[][]): number => {
    const place = new Array<number>(count).fill(0);
    for (const layer of ls) layer.forEach((i, p) => (place[i] = p));
    let total = 0;
    for (const layer of ls.slice(0, Math.max(ls.length - 1, 0))) {
      const pairs: [number, number][] = layer.flatMap((u) => down[u]!.map((v): [number, number] => [place[u]!, place[v]!]));
      for (let i = 0; i < pairs.length; i++) {
        for (let j = i + 1; j < pairs.length; j++) {
          const a = pairs[i]!;
          const b = pairs[j]!;
          if ((a[0] < b[0] && a[1] > b[1]) || (a[0] > b[0] && a[1] < b[1])) total += 1;
        }
      }
    }
    return total;
  };
  let best = layers.map((l) => [...l]);
  let bestCrossings = crossings(layers);
  for (let round = 0; round < 16; round++) {
    if (bestCrossings === 0) break;
    const forwardSweep = round % 2 === 0;
    const order: number[] = [];
    if (forwardSweep) for (let l = 1; l < layerCount; l++) order.push(l);
    else for (let l = layerCount - 2; l >= 0; l--) order.push(l);
    for (const l of order) {
      const neighbours = forwardSweep ? up : down;
      const place = new Array<number>(count).fill(0);
      for (const layer of layers) layer.forEach((i, p) => (place[i] = p));
      const keyed = layers[l]!.map((i): [number, number] => {
        const nb = neighbours[i]!;
        const key = nb.length === 0 ? place[i]! : nb.reduce((sum, j) => sum + place[j]!, 0) / nb.length;
        return [key, i];
      });
      keyed.sort((a, b) => byNumber(a[0], b[0]));
      layers[l] = keyed.map(([, i]) => i);
    }
    const c = crossings(layers);
    if (c < bestCrossings) {
      bestCrossings = c;
      best = layers.map((layer) => [...layer]);
    }
  }
  layers = best;

  // 6. Cross-axis coordinates.
  const separation = (a: number, b: number): number => {
    const half = (items[a]!.cross + items[b]!.cross) / 2;
    return half + (items[a]!.real !== null && items[b]!.real !== null ? NODE_GAP : DUMMY_GAP);
  };
  for (const layer of layers) {
    let at = 0;
    layer.forEach((i, k) => {
      if (k > 0) at += separation(layer[k - 1]!, i);
      items[i]!.pos = at;
    });
    const mid = at / 2;
    for (const i of layer) items[i]!.pos -= mid;
  }
  for (let round = 0; round < 16; round++) {
    const downward = round % 2 === 0;
    const order: number[] = [];
    if (downward) for (let l = 1; l < layerCount; l++) order.push(l);
    else for (let l = layerCount - 2; l >= 0; l--) order.push(l);
    for (const l of order) {
      const layer = layers[l]!;
      if (layer.length === 0) continue;
      const desired = layer.map((i) => {
        const nb = downward ? up[i]! : down[i]!;
        return nb.length === 0 ? items[i]!.pos : nb.reduce((sum, j) => sum + items[j]!.pos, 0) / nb.length;
      });
      const placed = new Array<number>(layer.length).fill(0);
      for (let k = 0; k < layer.length; k++) {
        placed[k] = k === 0 ? desired[0]! : Math.max(desired[k]!, placed[k - 1]! + separation(layer[k - 1]!, layer[k]!));
      }
      const shift = desired.reduce((sum, d, k) => sum + (d - placed[k]!), 0) / layer.length;
      layer.forEach((i, k) => (items[i]!.pos = placed[k]! + shift));
    }
  }

  // 7. Ports.
  const outOffset = new Array<number>(links.length).fill(0);
  const inOffset = new Array<number>(links.length).fill(0);
  for (let i = 0; i < n; i++) {
    const kind = graph.nodes[i]!.kind;
    if (kind === "decision" || kind === "circle") continue;
    const leaving: number[] = [];
    const entering: number[] = [];
    links.forEach((link, k) => {
      if (link.u === i) leaving.push(k);
      if (link.v === i) entering.push(k);
    });
    leaving.sort((a, b) => byNumber(items[links[a]!.v]!.pos, items[links[b]!.v]!.pos));
    entering.sort((a, b) => byNumber(items[links[a]!.u]!.pos, items[links[b]!.u]!.pos));
    for (const [group, offsets] of [[leaving, outOffset], [entering, inOffset]] as const) {
      const k = group.length;
      const step = Math.min(Math.max((items[i]!.cross - 16) / Math.max(k, 1), 0), 12);
      group.forEach((link, r) => (offsets[link] = (r - (k - 1) / 2) * step));
    }
  }
  const startOf = (k: number) => items[links[k]!.u]!.pos + outOffset[k]!;
  const endOf = (k: number) => items[links[k]!.v]!.pos + inOffset[k]!;
  const needsLane = (k: number) => Math.abs(startOf(k) - endOf(k)) > 1.5;

  // 8. Gaps and main coordinates.
  const lanesInGap = new Array<number>(layerCount).fill(0);
  for (let k = 0; k < links.length; k++) if (needsLane(k)) lanesInGap[items[links[k]!.u]!.layer]! += 1;
  const chips: (Chip | null)[] = graph.edges.map((e) => (e.label !== null && e.label.trim() !== "" ? chipOf(e.label, horizontal) : null));
  const gap = new Array<number>(layerCount).fill(RANK_GAP);
  for (let l = 0; l < layerCount; l++) gap[l] = Math.max(gap[l]!, (lanesInGap[l]! + 1) * LANE_SPACING + 20);
  const labelLink = new Array<number>(chains.length).fill(-1);
  chains.forEach((chain, c) => {
    const k = chain.links[Math.floor((chain.links.length - 1) / 2)]!;
    labelLink[c] = k;
    const chip = chips[chain.edge];
    if (chip) {
      const l = items[links[k]!.u]!.layer;
      gap[l] = Math.max(gap[l]!, chip.main + 2 * CHIP_MARGIN);
    }
  });
  const loopsAt = new Array<number>(n).fill(0);
  const loopSlot = new Array<number>(ends.length).fill(0);
  ends.forEach(([a, b], e) => {
    if (a !== b) return;
    loopSlot[e] = loopsAt[a]!;
    loopsAt[a]! += 1;
    const l = layerOf[a]!;
    const chip = chips[e];
    const reach = LOOP + 8 * loopsAt[a]! + (chip ? chip.main + 12 : 0);
    gap[l] = Math.max(gap[l]!, reach + 2 * CHIP_MARGIN);
  });
  const layerMain = new Array<number>(layerCount).fill(0);
  for (const item of items) layerMain[item.layer] = Math.max(layerMain[item.layer]!, item.main);
  const layerStart = new Array<number>(layerCount).fill(0);
  for (let l = 1; l < layerCount; l++) layerStart[l] = layerStart[l - 1]! + layerMain[l - 1]! + gap[l - 1]!;
  for (const item of items) item.m0 = layerStart[item.layer]! + (layerMain[item.layer]! - item.main) / 2;

  // 9. Routing.
  const laneX = new Array<number>(links.length).fill(Number.NaN);
  for (let l = 0; l < layerCount; l++) {
    const here: number[] = [];
    for (let k = 0; k < links.length; k++) if (items[links[k]!.u]!.layer === l && needsLane(k)) here.push(k);
    const downward = (k: number) => endOf(k) > startOf(k);
    here.sort((a, b) => {
      const da = downward(a);
      const db = downward(b);
      // Downward links first (largest start first), then upward ones.
      if (da !== db) return da ? -1 : 1;
      const ord = byNumber(startOf(a), startOf(b));
      return (da ? -ord : ord) || byNumber(a, b);
    });
    const g0 = layerStart[l]! + layerMain[l]!;
    const total = here.length;
    here.forEach((k, r) => (laneX[k] = g0 + (gap[l]! * (r + 1)) / (total + 1)));
  }
  const exitMain = (i: number) => items[i]!.m0 + items[i]!.main;
  const routes: Point[][] = graph.edges.map(() => []);
  for (const chain of chains) {
    const points: Point[] = [];
    for (const k of chain.links) {
      const link = links[k]!;
      const sm = exitMain(link.u);
      const em = items[link.v]!.m0;
      const sc = startOf(k);
      const ec = endOf(k);
      points.push([sm, sc]);
      if (Number.isFinite(laneX[k])) {
        points.push([laneX[k]!, sc], [laneX[k]!, ec], [em, ec]);
      } else {
        points.push([em, sc]);
      }
    }
    const cleaned = dedupe(points);
    if (chain.reversed) cleaned.reverse();
    routes[chain.edge] = cleaned;
  }
  const chipRects: [number, Box][] = [];
  const nodeBoxes: Box[] = Array.from({ length: n }, (_, i): Box => [items[i]!.m0, items[i]!.pos - items[i]!.cross / 2, items[i]!.main, items[i]!.cross]);
  const overlaps = (a: Box, b: Box, by: number) => a[0] - by < b[0] + b[2] && b[0] - by < a[0] + a[2] && a[1] - by < b[1] + b[3] && b[1] - by < a[1] + a[3];
  const placeChip = (edge: number, main: number, cross: number, chip: Chip) => {
    const step = chip.cross + 3;
    const candidate: Box = [main - chip.main / 2, cross - chip.cross / 2, chip.main, chip.cross];
    let chosen: Box = [...candidate];
    for (let k = 0; k < 12; k++) {
      const sign = k % 2 === 1 ? 1 : -1;
      const shift = sign * step * Math.floor((k + 1) / 2);
      candidate[1] = cross - chip.cross / 2 + shift;
      const blocked = chipRects.some(([, r]) => overlaps(candidate, r, 1)) || nodeBoxes.some((r) => overlaps(candidate, r, 2));
      if (!blocked) {
        chosen = [...candidate];
        break;
      }
    }
    chipRects.push([edge, chosen]);
  };
  chains.forEach((chain, c) => {
    const chip = chips[chain.edge];
    if (!chip) return;
    const k = labelLink[c]!;
    const l = items[links[k]!.u]!.layer;
    const g0 = layerStart[l]! + layerMain[l]!;
    const g1 = g0 + gap[l]!;
    const sc = startOf(k);
    const ec = endOf(k);
    let main: number;
    let cross: number;
    if (Number.isFinite(laneX[k])) {
      const lo = g0 + chip.main / 2 + 4;
      const hi = g1 - chip.main / 2 - 4;
      main = Math.min(Math.max(laneX[k]!, Math.min(lo, hi)), Math.max(hi, lo));
      cross = (sc + ec) / 2;
    } else {
      main = (g0 + g1) / 2;
      cross = sc;
    }
    placeChip(chain.edge, main, cross, chip);
  });
  ends.forEach(([a, b], e) => {
    if (a !== b) return;
    const item = items[a]!;
    const reach = LOOP + 8 * loopSlot[e]!;
    const quarter = item.cross * 0.3;
    const edgeMain = item.m0 + item.main;
    routes[e] = [
      [edgeMain, item.pos - quarter],
      [edgeMain + reach, item.pos - quarter],
      [edgeMain + reach, item.pos + quarter],
      [edgeMain, item.pos + quarter],
    ];
    const chip = chips[e];
    if (chip) placeChip(e, edgeMain + reach + chip.main / 2 + 6, item.pos, chip);
  });

  // 11. Map to screen space.
  const toScreenPoint = (m: number, c: number): Point => (horizontal ? [m, c] : [c, m]);
  const rectOf = (b: Box): Rect => (horizontal ? { x: b[0], y: b[1], w: b[2], h: b[3] } : { x: b[1], y: b[0], w: b[3], h: b[2] });
  const nodes: PlacedNode[] = Array.from({ length: n }, (_, i) => ({ index: i, rect: rectOf(nodeBoxes[i]!), lines: sized[i]!.lines }));
  const edges: PlacedEdge[] = graph.edges.map((_, e) => {
    const found = chipRects.find(([edge]) => edge === e);
    return {
      index: e,
      points: routes[e]!.map((p) => toScreenPoint(p[0], p[1])),
      label: found ? { rect: rectOf(found[1]), text: chips[e]?.text ?? "" } : null,
    };
  });
  // Bounds, mirroring and translation.
  const min: Point = [Number.MAX_VALUE, Number.MAX_VALUE];
  const max: Point = [-Number.MAX_VALUE, -Number.MAX_VALUE];
  const include = (x: number, y: number) => {
    min[0] = Math.min(min[0], x);
    min[1] = Math.min(min[1], y);
    max[0] = Math.max(max[0], x);
    max[1] = Math.max(max[1], y);
  };
  for (const node of nodes) {
    include(node.rect.x, node.rect.y);
    include(right(node.rect), bottom(node.rect));
  }
  for (const edge of edges) {
    for (const p of edge.points) include(p[0], p[1]);
    if (edge.label) {
      include(edge.label.rect.x, edge.label.rect.y);
      include(right(edge.label.rect), bottom(edge.label.rect));
    }
  }
  const width = max[0] - min[0] + 2 * PAD;
  const height = max[1] - min[1] + 2 * PAD;
  const direction: Direction = graph.direction;
  const flipX = direction === "RL";
  const flipY = direction === "BT";
  const map = (x: number, y: number): Point => [(flipX ? max[0] - x : x - min[0]) + PAD, (flipY ? max[1] - y : y - min[1]) + PAD];
  const mapRect = (r: Rect): Rect => {
    const a = map(r.x, r.y);
    const b = map(right(r), bottom(r));
    return { x: Math.min(a[0], b[0]), y: Math.min(a[1], b[1]), w: Math.abs(a[0] - b[0]), h: Math.abs(a[1] - b[1]) };
  };
  for (const node of nodes) node.rect = mapRect(node.rect);
  for (const edge of edges) {
    edge.points = edge.points.map((p) => map(p[0], p[1]));
    if (edge.label) edge.label.rect = mapRect(edge.label.rect);
  }
  return { width, height, nodes, edges };
}

/** Round the corners of a polyline: every interior vertex becomes a quadratic curve of at most `radius`. */
export function smooth(points: Point[], radius: number): Point[] {
  if (points.length < 3 || radius <= 0) return [...points];
  const out: Point[] = [points[0]!];
  for (let w = 1; w < points.length - 1; w++) {
    const a = points[w - 1]!;
    const b = points[w]!;
    const c = points[w + 1]!;
    const la = dist(a, b);
    const lc = dist(b, c);
    const r = Math.min(radius, la / 2, lc / 2);
    if (r < 0.5) {
      out.push(b);
      continue;
    }
    const p: Point = [b[0] + ((a[0] - b[0]) * r) / la, b[1] + ((a[1] - b[1]) * r) / la];
    const q: Point = [b[0] + ((c[0] - b[0]) * r) / lc, b[1] + ((c[1] - b[1]) * r) / lc];
    for (let step = 0; step <= 6; step++) {
      const t = step / 6;
      const s = 1 - t;
      out.push([s * s * p[0] + 2 * s * t * b[0] + t * t * q[0], s * s * p[1] + 2 * s * t * b[1] + t * t * q[1]]);
    }
  }
  out.push(points[points.length - 1]!);
  return out;
}

const dist = (a: Point, b: Point): number => Math.max(Math.hypot(a[0] - b[0], a[1] - b[1]), 1e-6);

/** The three corners of an arrowhead whose tip is `tip`, pointing away from `from` (the previous point of the line). */
export function arrowhead(tip: Point, from: Point, size: number): [Point, Point, Point] {
  const d = dist(tip, from);
  const ux = (tip[0] - from[0]) / d;
  const uy = (tip[1] - from[1]) / d;
  const base: Point = [tip[0] - ux * size, tip[1] - uy * size];
  const half = size * 0.45;
  return [tip, [base[0] - uy * half, base[1] + ux * half], [base[0] + uy * half, base[1] - ux * half]];
}
