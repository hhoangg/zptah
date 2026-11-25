// A laid-out flow diagram (Mermaid flowchart or `visual.flow`) in the shared viewer, drawn as React SVG elements from
// data only: labels are text children, never markup. Mirrors paint_flow in crates/monocode-gpui/src/diagram_viewer.rs.
import { useMemo } from "react";
import { alpha, useVisualColor } from "@/features/visuals/charts";
import { DiagramViewer } from "./DiagramViewer";
import { EDGE_FONT, LINE_HEIGHT, NODE_FONT, type Layout, type PlacedNode, arrowhead, layout, smooth } from "./layout";
import type { FlowGraph } from "./model";

const EDGE_RADIUS = 8;
const ARROW = 9;
const MONO = "ui-monospace, Menlo, monospace";
const LINE = alpha("var(--content)", 62);

type Laid = { ok: true; layout: Layout } | { ok: false; error: string };

/** The layout of a graph, or the reason it cannot be drawn. */
export function useFlowLayout(graph: FlowGraph): Laid {
  return useMemo(() => {
    try {
      return { ok: true, layout: layout(graph) };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : "invalid diagram" };
    }
  }, [graph]);
}

const pointList = (points: readonly (readonly [number, number])[]) => points.map((p) => `${p[0]},${p[1]}`).join(" ");
const pathOf = (points: readonly (readonly [number, number])[]) => points.map((p, i) => `${i === 0 ? "M" : "L"}${p[0]} ${p[1]}`).join(" ");

function NodeShape({ graph, node }: { graph: FlowGraph; node: PlacedNode }) {
  const color = useVisualColor();
  const spec = graph.nodes[node.index]!;
  const { rect } = node;
  const accent = spec.color ? color(spec.color) : null;
  const stroke = accent ?? alpha("var(--content)", 60);
  const tint = accent ? alpha(accent, 16) : alpha("var(--content)", 5);
  const nominal = spec.kind === "box" ? 5 : spec.kind === "round" ? 12 : spec.kind === "decision" ? 0 : Math.max(rect.w, rect.h);
  const radius = Math.min(nominal, rect.w / 2, rect.h / 2);
  const top = rect.y + (rect.h - node.lines.length * LINE_HEIGHT) / 2;
  return (
    <g data-flow-node={spec.id} data-kind={spec.kind}>
      {spec.kind === "decision" ? (
        (() => {
          const cx = rect.x + rect.w / 2;
          const cy = rect.y + rect.h / 2;
          const diamond = pointList([[cx, rect.y], [rect.x + rect.w, cy], [cx, rect.y + rect.h], [rect.x, cy]]);
          return (
            <>
              <polygon points={diamond} fill="var(--surface-raised)" />
              <polygon points={diamond} fill={tint} stroke={stroke} strokeWidth={1.5} strokeLinejoin="round" />
            </>
          );
        })()
      ) : (
        <>
          <rect x={rect.x} y={rect.y} width={rect.w} height={rect.h} rx={radius} fill="var(--surface-raised)" />
          <rect x={rect.x} y={rect.y} width={rect.w} height={rect.h} rx={radius} fill={tint} stroke={stroke} strokeWidth={1.5} />
        </>
      )}
      {node.lines.map((line, i) => (
        <text key={i} x={rect.x + rect.w / 2} y={top + (i + 0.5) * LINE_HEIGHT} textAnchor="middle" dominantBaseline="central" fill="var(--content)" fontSize={NODE_FONT} fontFamily={MONO}>
          {line}
        </text>
      ))}
    </g>
  );
}

/** The SVG of a laid-out graph, in content coordinates. */
export function FlowShapes({ graph, laid }: { graph: FlowGraph; laid: Layout }) {
  return (
    <g data-testid="flow-shapes">
      {laid.edges.map((edge) => {
        const spec = graph.edges[edge.index]!;
        const n = edge.points.length;
        const head = spec.arrow && n >= 2 ? arrowhead(edge.points[n - 1]!, edge.points[n - 2]!, ARROW) : null;
        return (
          <g key={edge.index} data-flow-edge={edge.index} data-dashed={spec.dashed || undefined}>
            <path d={pathOf(smooth(edge.points, EDGE_RADIUS))} fill="none" stroke={LINE} strokeWidth={1.6} strokeLinejoin="round" strokeDasharray={spec.dashed ? "7 5" : undefined} />
            {head ? <polygon points={pointList(head)} fill={LINE} /> : null}
          </g>
        );
      })}
      {laid.nodes.map((node) => (
        <NodeShape key={node.index} graph={graph} node={node} />
      ))}
      {laid.edges.map((edge) =>
        edge.label ? (
          <g key={`label-${edge.index}`} data-flow-label={edge.index}>
            <rect x={edge.label.rect.x} y={edge.label.rect.y} width={edge.label.rect.w} height={edge.label.rect.h} rx={4} fill="var(--surface-raised)" stroke={alpha("var(--content)", 20)} strokeWidth={1} />
            <text x={edge.label.rect.x + edge.label.rect.w / 2} y={edge.label.rect.y + edge.label.rect.h / 2} textAnchor="middle" dominantBaseline="central" fill={alpha("var(--content)", 95)} fontSize={EDGE_FONT} fontFamily={MONO}>
              {edge.label.text}
            </text>
          </g>
        ) : null,
      )}
    </g>
  );
}

export interface FlowDiagramProps {
  graph: FlowGraph;
  title: string;
  ariaLabel: string;
  boardHeight?: number | null;
  source?: string;
  sourceLanguage?: string;
  testId?: string;
}

/** A flow graph in the shared viewer; when it cannot be laid out, says why (and offers the source if there is one). */
export function FlowDiagram(props: FlowDiagramProps) {
  const laid = useFlowLayout(props.graph);
  if (!laid.ok) {
    return (
      <p role="alert" className="text-caption text-[color:var(--viz-negative)]">
        Unable to draw diagram: {laid.error}
      </p>
    );
  }
  return (
    <DiagramViewer
      title={props.title}
      ariaLabel={props.ariaLabel}
      width={laid.layout.width}
      height={laid.layout.height}
      boardHeight={props.boardHeight ?? null}
      {...(props.source !== undefined ? { source: props.source } : {})}
      {...(props.sourceLanguage ? { sourceLanguage: props.sourceLanguage } : {})}
      {...(props.testId ? { testId: props.testId } : {})}
    >
      <FlowShapes graph={props.graph} laid={laid.layout} />
    </DiagramViewer>
  );
}

export default FlowDiagram;
