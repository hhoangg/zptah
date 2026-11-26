// The `mermaid` package splits into about two hundred chunks (one per diagram type and per layout engine, about 5 MiB) that
// only `features/diagram/mermaidRender.ts` reaches, through a dynamic import. Workbox would precache every emitted file, so
// each user would download all of it when the service worker installs, defeating the lazy import. This plugin finds the chunks
// that only the mermaid import reaches and keeps them out of the precache manifest (`injectManifest.manifestTransforms` in
// vite.config.ts): they are fetched, and cached by the browser, the first time a diagram is drawn. Offline, drawing then
// fails and the block shows its source with a note.
import type { Plugin } from "vite";

interface ChunkInfo {
  fileName: string;
  imports: string[];
  dynamicImports: string[];
  moduleIds: string[];
}

/** Files (as in the bundle, `assets/x.js`) reachable only through the dynamic import made by the chunk holding `marker`. */
export function lazyOnlyChunks(chunks: readonly ChunkInfo[], entries: readonly string[], marker: string): Set<string> {
  const byName = new Map(chunks.map((chunk) => [chunk.fileName, chunk]));
  const holder = chunks.find((chunk) => chunk.moduleIds.some((id) => id.endsWith(marker)));
  if (!holder) return new Set();
  const roots = new Set(holder.dynamicImports);
  const walk = (starts: Iterable<string>, skip: ReadonlySet<string>): Set<string> => {
    const seen = new Set<string>();
    const stack = [...starts].filter((name) => !skip.has(name));
    while (stack.length > 0) {
      const name = stack.pop() as string;
      if (seen.has(name)) continue;
      seen.add(name);
      const chunk = byName.get(name);
      if (chunk) for (const next of [...chunk.imports, ...chunk.dynamicImports]) if (!skip.has(next) && !seen.has(next)) stack.push(next);
    }
    return seen;
  };
  const elsewhere = walk(entries, roots);
  return new Set([...walk(roots, new Set())].filter((name) => !elsewhere.has(name)));
}

export function lazyMermaidPrecache(): { plugin: Plugin; isLazy: (url: string) => boolean } {
  let lazy = new Set<string>();
  const plugin: Plugin = {
    name: "lazy-mermaid-precache",
    apply: "build",
    applyToEnvironment: (environment) => environment.name === "client",
    generateBundle(_options, bundle) {
      const chunks: ChunkInfo[] = [];
      const entries: string[] = [];
      for (const item of Object.values(bundle)) {
        if (item.type !== "chunk") continue;
        chunks.push({ fileName: item.fileName, imports: item.imports, dynamicImports: item.dynamicImports, moduleIds: item.moduleIds });
        if (item.isEntry) entries.push(item.fileName);
      }
      lazy = lazyOnlyChunks(chunks, entries, "/src/features/diagram/mermaidRender.ts");
    },
  };
  return { plugin, isLazy: (url) => lazy.has(url.replace(/^\//, "")) };
}
