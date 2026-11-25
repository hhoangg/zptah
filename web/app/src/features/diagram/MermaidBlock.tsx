// A ```mermaid fence of a reply. The flowchart subset (the same one the desktop draws) is parsed, laid out and shown in
// the shared diagram viewer. Everything else, and a block that is still streaming, shows its source as a code block:
// mermaid.js is not loaded because it builds its output with innerHTML, which this app's Trusted Types policy and CSP
// refuse by design (security.md section 8), so there is no safe way to put its output on the page.
import { useMemo } from "react";
import { CodeBlock } from "@/features/markdown/CodeBlock";
import { FlowDiagram } from "./FlowDiagram";
import { parseFlowchart } from "./mermaid";

export function MermaidBlock({ code, isIncomplete = false }: { code: string; isIncomplete?: boolean }) {
  const source = code.replace(/\n+$/, "");
  const parsed = useMemo(() => (isIncomplete ? null : parseFlowchart(source)), [source, isIncomplete]);
  if (!parsed) return <CodeBlock code={code} language="mermaid" isIncomplete />;
  if (!parsed.ok) {
    return (
      <div data-testid="mermaid-fallback">
        <CodeBlock code={code} language="mermaid" />
        <p className="px-1 pt-1 text-caption text-[color:var(--content-a50)]">Shown as source: this diagram is not a simple flowchart, which is all the web app draws ({parsed.error}).</p>
      </div>
    );
  }
  return <FlowDiagram graph={parsed.graph} title="Mermaid" ariaLabel="Mermaid flowchart" source={source} sourceLanguage="mermaid" testId="mermaid-diagram" />;
}

export default MermaidBlock;
