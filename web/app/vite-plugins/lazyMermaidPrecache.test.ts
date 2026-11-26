import { describe, expect, it } from "vitest";
import { lazyOnlyChunks } from "./lazyMermaidPrecache";

const chunk = (fileName: string, imports: string[] = [], dynamicImports: string[] = [], moduleIds: string[] = []) => ({ fileName, imports, dynamicImports, moduleIds });
const MARKER = "/src/features/diagram/mermaidRender.ts";

describe("lazyOnlyChunks", () => {
  const chunks = [
    chunk("assets/index.js", ["assets/shared.js"], ["assets/block.js"]),
    chunk("assets/block.js", ["assets/shared.js"], ["assets/mermaid.js"], ["/app/src/features/diagram/mermaidRender.ts"]),
    chunk("assets/mermaid.js", ["assets/shared.js", "assets/d3.js"], ["assets/sequence.js"]),
    chunk("assets/sequence.js", ["assets/d3.js", "assets/only-seq.js"]),
    chunk("assets/only-seq.js"),
    chunk("assets/d3.js"),
    chunk("assets/shared.js"),
  ];

  it("returns the chunks only the mermaid import reaches, not the ones the app also uses", () => {
    expect([...lazyOnlyChunks(chunks, ["assets/index.js"], MARKER)].sort()).toEqual(["assets/d3.js", "assets/mermaid.js", "assets/only-seq.js", "assets/sequence.js"]);
  });

  it("keeps a chunk the app reaches another way precached", () => {
    const other = [chunk("assets/index.js", ["assets/shared.js"], ["assets/block.js", "assets/other.js"]), ...chunks.slice(1), chunk("assets/other.js", ["assets/d3.js"])];
    const lazy = lazyOnlyChunks(other, ["assets/index.js"], MARKER);
    expect(lazy.has("assets/d3.js")).toBe(false);
    expect(lazy.has("assets/mermaid.js")).toBe(true);
  });

  it("returns nothing when no chunk holds the marker", () => {
    expect(lazyOnlyChunks([chunk("assets/index.js")], ["assets/index.js"], "/x.ts").size).toBe(0);
  });
});
