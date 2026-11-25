// The diagram code (layout, parser, viewer) is a separate chunk: a transcript without a diagram never downloads it.
import { type ComponentProps, Suspense, lazy } from "react";
import { CodeBlock } from "@/features/markdown/CodeBlock";

export const isMermaidLanguage = (lang: string): boolean => lang.trim().toLowerCase() === "mermaid";

const Mermaid = lazy(() => import("./MermaidBlock"));
const Flow = lazy(() => import("./FlowDiagram"));

export function LazyMermaidBlock(props: ComponentProps<typeof Mermaid>) {
  return (
    <Suspense fallback={<CodeBlock code={props.code} language="mermaid" isIncomplete />}>
      <Mermaid {...props} />
    </Suspense>
  );
}

export function LazyFlowDiagram(props: ComponentProps<typeof Flow>) {
  return (
    <Suspense fallback={<div className="h-[180px] rounded-card border border-stroke bg-[var(--content-a04)]" aria-busy="true" />}>
      <Flow {...props} />
    </Suspense>
  );
}
