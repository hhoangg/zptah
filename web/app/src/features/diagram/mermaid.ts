// A parser for the common subset of Mermaid flowcharts, ported from crates/monocode-core/src/diagram/mermaid.rs. It
// produces the shared `FlowGraph`. Anything outside the subset (subgraphs, styling, class shorthand, `&` groups, exotic
// shapes or arrows, other diagram types) is an error and the caller shows the source instead. Input is only ever read
// as text: nothing here builds HTML.
import { type Direction, type FlowEdge, type FlowGraph, type NodeKind, parseDirection, validateGraph } from "./model";

export type ParseResult = { ok: true; graph: FlowGraph } | { ok: false; error: string };

class Unsupported extends Error {}
const fail = (message: string): never => {
  throw new Unsupported(message);
};

export function parseFlowchart(source: string): ParseResult {
  try {
    return { ok: true, graph: parse(source) };
  } catch (error) {
    if (error instanceof Unsupported) return { ok: false, error: error.message };
    throw error;
  }
}

const words = (text: string) => text.split(/\s+/).filter(Boolean);

function parse(source: string): FlowGraph {
  const lines = source
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter((l) => l !== "" && !l.startsWith("%%"));
  const first = lines.shift();
  if (first === undefined) return fail("Empty diagram");
  // `graph TD; A-->B` carries statements after the header on one line.
  const head = splitStatements(first);
  const header = head.shift();
  if (header === undefined) return fail("Empty diagram");
  const parts = words(header);
  if (parts[0] !== "flowchart" && parts[0] !== "graph") return fail("Not a flowchart");
  let direction: Direction = "TB";
  if (parts[1] !== undefined) direction = parseDirection(parts[1]) ?? fail("Unsupported flowchart direction");
  if (parts.length > 2) return fail("Unsupported flowchart header");
  const graph: FlowGraph = { direction, nodes: [], edges: [] };
  const index = new Map<string, number>();
  for (const statement of head) parseStatement(statement, graph, index);
  for (const line of lines) for (const statement of splitStatements(line)) parseStatement(statement, graph, index);
  const invalid = validateGraph(graph);
  if (invalid) return fail(invalid);
  return graph;
}

/** Split on `;` that is outside quotes and brackets. */
function splitStatements(line: string): string[] {
  const out: string[] = [];
  let current = "";
  let quoted = false;
  let depth = 0;
  for (const c of line) {
    if (c === '"') quoted = !quoted;
    else if (!quoted && (c === "[" || c === "(" || c === "{")) depth += 1;
    else if (!quoted && (c === "]" || c === ")" || c === "}")) depth -= 1;
    else if (c === ";" && !quoted && depth <= 0) {
      out.push(current);
      current = "";
      continue;
    }
    current += c;
  }
  out.push(current);
  return out.map((s) => s.trim()).filter((s) => s !== "");
}

class Cursor {
  readonly chars: string[];
  at = 0;
  constructor(text: string) {
    this.chars = Array.from(text);
  }
  restStarts(text: string): boolean {
    return Array.from(text).every((c, i) => this.chars[this.at + i] === c);
  }
  peek(): string | undefined {
    return this.chars[this.at];
  }
  skipSpace(): void {
    while (this.at < this.chars.length && /\s/u.test(this.chars[this.at]!)) this.at += 1;
  }
  slice(from: number, to: number): string {
    return this.chars.slice(from, to).join("");
  }
}

const KEYWORDS = ["subgraph", "end", "classDef", "class", "style", "linkStyle", "click", "direction"];

function parseStatement(statement: string, graph: FlowGraph, index: Map<string, number>): void {
  const first = words(statement)[0] ?? "";
  if (KEYWORDS.includes(first)) fail(`Unsupported flowchart statement '${first}'`);
  const cur = new Cursor(statement);
  let previous = parseNode(cur, graph, index);
  for (;;) {
    cur.skipSpace();
    if (cur.peek() === undefined) return;
    const link = parseLink(cur);
    cur.skipSpace();
    const next = parseNode(cur, graph, index);
    graph.edges.push({ from: graph.nodes[previous]!.id, to: graph.nodes[next]!.id, label: link.label, dashed: link.dashed, arrow: link.arrow } satisfies FlowEdge);
    previous = next;
  }
}

const isIdChar = (c: string | undefined) => c !== undefined && (c === "_" || /[\p{Alphabetic}\p{N}]/u.test(c));

function parseNode(cur: Cursor, graph: FlowGraph, index: Map<string, number>): number {
  const start = cur.at;
  while (isIdChar(cur.peek())) cur.at += 1;
  if (cur.at === start) return fail("Unsupported flowchart syntax");
  const id = cur.slice(start, cur.at);
  const shape = parseShape(cur);
  if (cur.restStarts(":::") || cur.peek() === "&") return fail("Unsupported flowchart syntax");
  let slot = index.get(id);
  if (slot === undefined) {
    graph.nodes.push({ id, label: id, kind: "box", color: null });
    slot = graph.nodes.length - 1;
    index.set(id, slot);
  }
  if (shape) {
    graph.nodes[slot]!.kind = shape.kind;
    graph.nodes[slot]!.label = shape.label;
  }
  return slot;
}

const SHAPES: [string, string, NodeKind][] = [
  ["([", "])", "start"],
  ["((", "))", "circle"],
  ["[", "]", "box"],
  ["(", ")", "round"],
];

function parseShape(cur: Cursor): { kind: NodeKind; label: string } | null {
  if (cur.peek() === "{") {
    if (cur.restStarts("{{")) return fail("Unsupported flowchart shape");
    return { kind: "decision", label: cleanLabel(readUntil(cur, 1, "}")) };
  }
  for (const [open, close, kind] of SHAPES) {
    if (cur.restStarts(open)) return { kind, label: cleanLabel(readUntil(cur, Array.from(open).length, close)) };
  }
  if (cur.peek() === ">") return fail("Unsupported flowchart shape");
  return null;
}

/** Consume `skip` opening characters, then text up to `close` (outside quotes). */
function readUntil(cur: Cursor, skip: number, close: string): string {
  cur.at += skip;
  const start = cur.at;
  let quoted = false;
  while (cur.at < cur.chars.length) {
    if (cur.chars[cur.at] === '"') quoted = !quoted;
    else if (!quoted && cur.restStarts(close)) {
      const text = cur.slice(start, cur.at);
      cur.at += Array.from(close).length;
      return text;
    }
    cur.at += 1;
  }
  return fail("Unterminated flowchart node text");
}

function cleanLabel(raw: string): string {
  let text = raw.trim();
  if (text.startsWith('"')) {
    const inner = text.slice(1);
    if (inner.endsWith('"')) text = inner.slice(0, -1);
  }
  for (const br of ["<br/>", "<br />", "<br>"]) text = text.split(br).join("\n");
  return text.split("&quot;").join('"').split("&amp;").join("&").split("&lt;").join("<").split("&gt;").join(">");
}

function parseLink(cur: Cursor): { dashed: boolean; arrow: boolean; label: string | null } {
  const c = cur.peek();
  if (c === undefined) return fail("Unsupported flowchart link");
  let dashed = false;
  let arrow = false;
  let label: string | null = null;
  if (c === "-" && cur.restStarts("-.")) {
    // Dotted: -.-> , -.- , or -. text .->
    cur.at += 2;
    dashed = true;
    if (cur.peek() === " ") {
      const taken = takeUntilAny(cur, [".->", ".-"]);
      arrow = taken.arrow;
      label = taken.text;
    } else {
      while (cur.peek() === ".") cur.at += 1;
      if (cur.peek() !== "-") return fail("Unsupported flowchart link");
      cur.at += 1;
      if (cur.peek() === ">") {
        cur.at += 1;
        arrow = true;
      }
    }
  } else if (c === "-" || c === "=") {
    let count = 0;
    while (cur.peek() === c) {
      cur.at += 1;
      count += 1;
    }
    if (count < 2) return fail("Unsupported flowchart link");
    if (cur.peek() === ">") {
      cur.at += 1;
      arrow = true;
    } else if (count >= 3) {
      arrow = false;
    } else if (cur.peek() === " ") {
      // `-- text -->`, `-- text ---`, `== text ==>`
      const taken = takeUntilAny(cur, c === "-" ? ["-->", "---"] : ["==>", "==="]);
      arrow = taken.arrow;
      label = taken.text;
    } else {
      return fail("Unsupported flowchart link");
    }
  } else {
    return fail("Unsupported flowchart link");
  }
  cur.skipSpace();
  if (cur.peek() === "|") label = cleanLabel(readUntil(cur, 1, "|"));
  return { dashed, arrow, label: label !== null && label.trim() !== "" ? label : null };
}

/** Read link text up to the first closer; a closer ending in `>` is an arrow. */
function takeUntilAny(cur: Cursor, closers: string[]): { text: string; arrow: boolean } {
  const start = cur.at;
  while (cur.at < cur.chars.length) {
    for (const closer of closers) {
      if (cur.restStarts(closer)) {
        const text = cur.slice(start, cur.at);
        cur.at += Array.from(closer).length;
        return { text: cleanLabel(text), arrow: closer.endsWith(">") };
      }
    }
    cur.at += 1;
  }
  return fail("Unterminated flowchart link text");
}
