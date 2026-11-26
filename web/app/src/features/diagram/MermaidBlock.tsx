// A ```mermaid fence of a reply, drawn with the official `mermaid` library (every diagram type) in the shared diagram
// viewer. The library is lazy: this chunk and `mermaid` load only when a reply holds a closed mermaid block.
//
// Mermaid's SVG never becomes page markup (security.md 8, rule 7). The string is turned into a `blob:` image that the
// viewer draws with an SVG <image>: an SVG used as an image runs no script, loads nothing external, and its own <style> is
// not subject to the page's `style-src`. Source shows while the block is still streaming, while the library loads, and
// whenever the diagram cannot be drawn (a parse error, too large, an image the browser refuses), with a short note.
import { useEffect, useMemo, useState } from "react";
import { CodeBlock } from "@/features/markdown/CodeBlock";
import { useAppearance } from "@/lib/appearance";
import { DiagramViewer } from "./DiagramViewer";
import { renderMermaid } from "./mermaidRender";

/** The desktop previews at most 32 KiB and 512 lines of Mermaid (monocode-gpui markdown/mermaid.rs); the same bounds here. */
export const MAX_MERMAID_BYTES = 32 * 1024;
export const MAX_MERMAID_LINES = 512;
const tooLarge = (source: string) => new TextEncoder().encode(source).length > MAX_MERMAID_BYTES || source.split("\n").length > MAX_MERMAID_LINES;

type Drawing = { kind: "pending" } | { kind: "drawn"; url: string; width: number; height: number } | { kind: "failed"; error: string };

/** Resolves when the browser can decode the image (a drawing it refuses would show as an empty board). */
function loads(url: string): Promise<boolean> {
  return new Promise((resolve) => {
    const image = new Image();
    image.onload = () => resolve(true);
    image.onerror = () => resolve(false);
    image.src = url;
  });
}

export function MermaidBlock({ code, isIncomplete = false }: { code: string; isIncomplete?: boolean }) {
  const source = code.replace(/\n+$/, "");
  const scheme = useAppearance((state) => state.scheme);
  const oversized = useMemo(() => tooLarge(source), [source]);
  const [drawing, setDrawing] = useState<Drawing>({ kind: "pending" });
  const skip = isIncomplete || oversized;

  useEffect(() => {
    if (skip) return;
    let alive = true;
    void (async () => {
      const result = await renderMermaid(source, scheme, MAX_MERMAID_BYTES);
      if (!alive) return;
      if (!result.ok) {
        setDrawing({ kind: "failed", error: result.error });
        return;
      }
      const url = URL.createObjectURL(new Blob([result.svg], { type: "image/svg+xml" }));
      const loaded = await loads(url);
      if (!alive) {
        // The block went away or its input changed while the browser decoded the image: nothing owns the URL.
        URL.revokeObjectURL(url);
        return;
      }
      if (!loaded) {
        URL.revokeObjectURL(url);
        setDrawing({ kind: "failed", error: "the browser could not show the result" });
        return;
      }
      setDrawing({ kind: "drawn", url, width: result.width, height: result.height });
    })();
    return () => {
      alive = false;
    };
  }, [source, scheme, skip]);

  // A replaced or unmounted drawing releases its blob.
  useEffect(
    () => () => {
      if (drawing.kind === "drawn") URL.revokeObjectURL(drawing.url);
    },
    [drawing],
  );

  if (isIncomplete || (!oversized && drawing.kind === "pending")) return <CodeBlock code={code} language="mermaid" isIncomplete />;
  if (oversized || drawing.kind === "failed") {
    const reason = oversized ? "this diagram is too large to draw" : `could not draw this diagram (${drawing.kind === "failed" ? drawing.error : ""})`;
    return (
      <div data-testid="mermaid-fallback">
        <CodeBlock code={code} language="mermaid" />
        <p className="px-1 pt-1 text-caption text-[color:var(--content-a50)]">Shown as source: {reason}.</p>
      </div>
    );
  }
  if (drawing.kind !== "drawn") return null;
  return (
    <DiagramViewer title="Mermaid" ariaLabel="Mermaid diagram" width={drawing.width} height={drawing.height} source={source} sourceLanguage="mermaid" testId="mermaid-diagram">
      <image href={drawing.url} x={0} y={0} width={drawing.width} height={drawing.height} data-testid="mermaid-image" />
    </DiagramViewer>
  );
}

export default MermaidBlock;
