import { describe, expect, it } from "vitest";
import { layout } from "./layout";
import { parseFlowchart } from "./mermaid";
import type { FlowGraph } from "./model";

const ok = (source: string): FlowGraph => {
  const result = parseFlowchart(source);
  if (!result.ok) throw new Error(result.error);
  return result.graph;
};

describe("mermaid flowchart subset", () => {
  it("parses nodes, shapes, link styles and labels to the shared model", () => {
    const g = ok("flowchart LR\n  A[Mono request] --> B{Risk?}\n  B -- safe --> C[Auto act]\n  B -->|risky| D([Ask user])\n  D -.-> E((Done))\n  C ==> E\n  E --- A\n");
    expect(g.direction).toBe("LR");
    expect(g.nodes.map((n) => [n.id, n.label, n.kind])).toEqual([
      ["A", "Mono request", "box"],
      ["B", "Risk?", "decision"],
      ["C", "Auto act", "box"],
      ["D", "Ask user", "start"],
      ["E", "Done", "circle"],
    ]);
    expect(g.edges).toHaveLength(6);
    expect(g.edges[1]!.label).toBe("safe");
    expect(g.edges[2]!.label).toBe("risky");
    expect(g.edges[3]).toMatchObject({ dashed: true, arrow: true });
    expect(g.edges[5]!.arrow).toBe(false);
    expect(() => layout(g)).not.toThrow();
  });

  it("handles chains, semicolons, comments and line breaks", () => {
    const g = ok('graph TD;\n%% a comment\n%% another\nA["Line one<br/>line two"]-->B-->C; C-->A\n');
    expect(ok("graph LR; X-->Y").edges).toHaveLength(1);
    expect(g.direction).toBe("TB");
    expect(g.nodes[0]!.label).toBe("Line one\nline two");
    expect(g.edges).toHaveLength(3);
    expect(g.nodes).toHaveLength(3);
    // A later definition renames an earlier bare node.
    expect(ok("flowchart TB\nA --> B\nB[Bee]\n").nodes[1]!.label).toBe("Bee");
  });

  it("reads dotted links with text, the default direction, and entities", () => {
    expect(ok("flowchart\nA -. maybe .-> B").edges[0]).toMatchObject({ label: "maybe", dashed: true, arrow: true });
    expect(ok("flowchart\nA --> B").direction).toBe("TB");
    expect(ok('flowchart LR\nA["a &lt;b&gt; &amp; &quot;c&quot;"] --> B').nodes[0]!.label).toBe('a <b> & "c"');
  });

  it("rejects anything outside the subset so the caller can show the source", () => {
    for (const source of [
      "sequenceDiagram\nA->>B: hi",
      "flowchart LR\nsubgraph s\nA-->B\nend",
      "flowchart LR\nA-->B\nstyle A fill:#f00",
      "flowchart LR\nA-->B & C",
      "flowchart LR\nA:::hot-->B",
      "flowchart LR\nA{{hex}}-->B",
      "flowchart LR\nA-->",
      "flowchart sideways\nA-->B",
      "",
    ]) {
      expect(parseFlowchart(source).ok, `accepted ${JSON.stringify(source)}`).toBe(false);
    }
  });

  it("keeps hostile text as plain label text", () => {
    const g = ok('flowchart LR\nA["<script>alert(1)</script>"] --> B[<img src=x onerror=alert(1)>]');
    expect(g.nodes[0]!.label).toBe("<script>alert(1)</script>");
    expect(g.nodes[1]!.label).toBe("<img src=x onerror=alert(1)>");
  });
});
