// The shared diagram model, ported from crates/monocode-core/src/diagram.rs: one graph type for Mermaid flowcharts and
// the native `visual.flow` node, plus the canvas label anchoring math. Pure data and functions, no DOM.
import type { ThemeColor } from "@/features/visuals/types";

export const MAX_FLOW_NODES = 120;
export const MAX_FLOW_EDGES = 400;
const MAX_ID_CHARS = 64;

export type Direction = "LR" | "RL" | "TB" | "BT";
export type NodeKind = "box" | "round" | "decision" | "start" | "end" | "circle";

export type FlowNode = { id: string; label: string; kind: NodeKind; color: ThemeColor | null };
export type FlowEdge = { from: string; to: string; label: string | null; dashed: boolean; arrow: boolean };
export type FlowGraph = { direction: Direction; nodes: FlowNode[]; edges: FlowEdge[] };

export const isHorizontal = (direction: Direction): boolean => direction === "LR" || direction === "RL";

export function parseDirection(text: string): Direction | null {
  switch (text) {
    case "LR":
    case "RL":
    case "BT":
      return text;
    case "TB":
    case "TD":
      return "TB";
    default:
      return null;
  }
}

/** Structural validation with errors an author can act on; returns the message or null. */
export function validateGraph(graph: FlowGraph): string | null {
  if (graph.nodes.length === 0 || graph.nodes.length > MAX_FLOW_NODES) return `Flow needs 1-${MAX_FLOW_NODES} nodes`;
  if (graph.edges.length > MAX_FLOW_EDGES) return `Flow allows at most ${MAX_FLOW_EDGES} edges`;
  const seen = new Set<string>();
  for (const node of graph.nodes) {
    const chars = [...node.id].length;
    if (chars === 0 || chars > MAX_ID_CHARS || /\p{Cc}/u.test(node.id)) {
      return `Flow node ids must contain 1-${MAX_ID_CHARS} printable characters, got ${JSON.stringify(node.id)}`;
    }
    if (seen.has(node.id)) return `Duplicate flow node id '${node.id}'`;
    seen.add(node.id);
  }
  for (const [i, edge] of graph.edges.entries()) {
    for (const [side, id] of [["from", edge.from], ["to", edge.to]] as const) {
      if (!seen.has(id)) return `Flow edge ${i} has unknown node id '${id}' in ${side}; declare it in nodes`;
    }
  }
  return null;
}

// --- Canvas labels ---

export type TextAlign = "start" | "center" | "end";
export type Baseline = "top" | "middle" | "bottom" | "alphabetic";

/** Canvas label font size and line box, in canvas units. */
export const LABEL_FONT = 12;
export const LABEL_LINE = LABEL_FONT * 1.4;
/** Distance from the top of a line box to its baseline, as a fraction of the font size. */
export const ASCENT = 1;

/** The top-left corner of a label whose anchor is `(x, y)`. */
export function labelOrigin(x: number, y: number, textWidth: number, font: number, align: TextAlign, baseline: Baseline): [number, number] {
  const line = font * 1.4;
  const left = align === "start" ? x : align === "center" ? x - textWidth / 2 : x - textWidth;
  const top = baseline === "top" ? y : baseline === "middle" ? y - line / 2 : baseline === "bottom" ? y - line : y - ASCENT * font;
  return [left, top];
}

/** WCAG relative luminance of an sRGB colour with components in 0..=1. */
export function luminance(rgb: [number, number, number]): number {
  const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
  return 0.2126 * lin(rgb[0]) + 0.7152 * lin(rgb[1]) + 0.0722 * lin(rgb[2]);
}

/** Whether dark text reads better than light text on a fill of this luminance (0.179: black and white contrast equally). */
export const prefersDarkText = (fillLuminance: number): boolean => fillLuminance > 0.179;
