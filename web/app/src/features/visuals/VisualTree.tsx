// F-73: a visual's component tree drawn as React elements and inline SVG, as the desktop's VisualView lays it
// out (ported from mobile/src/features/visuals/VisualTree.tsx at 8b4d76ae). The tree is data from the desktop,
// never HTML: text goes through React children, so a hostile string is only text. The visual's program runs on
// the desktop, never in the browser (no eval, see web/docs/security.md), so a control here only reports what the
// user did through `onDispatch(handler, value)` and the host answers with the next tree. Without `onDispatch`
// (an older desktop, a document dialog) the controls are drawn disabled with one hint per visual. 3D scenes are
// not drawn.
import { Component, createContext, use, useEffect, useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Cartesian, Donut, Gauge, Heatmap, Progress, Trend, alpha, useVisualColor } from "./charts";
import { LazyFlowDiagram } from "@/features/diagram/lazy";
import { CanvasShapes } from "./Canvas";
import { MAX_INPUT_BYTES, controlsOf, utf8Length, type VisualEventValue } from "./controls";
import type { TextSize, VisualNode } from "./types";

export { parseVisualTree } from "./parse";

const TEXT_CLASS: Record<TextSize, string> = {
  caption: "text-[11px] leading-4",
  small: "text-small",
  body: "text-body",
  subtitle: "text-[17px] leading-6 font-semibold",
  title: "text-[22px] leading-7 font-semibold",
  hero: "text-[34px] leading-10 font-semibold",
};

/** A node the renderer cannot draw (data the parser let through but a chart cannot lay out) must not take the page down. */
class VisualBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  override state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  override render() {
    return this.state.failed ? <p className="text-small text-muted-text">This visual could not be drawn. Open it on your desktop to see it.</p> : this.props.children;
  }
}

/** What the controls of a tree do. `dispatch` is undefined when nothing can act on them. */
const ControlsContext = createContext<{ dispatch: ((handler: string, value: VisualEventValue) => void) | undefined }>({ dispatch: undefined });

export const OPEN_ON_DESKTOP_HINT = "Open on desktop to interact";

export interface VisualTreeProps {
  node: VisualNode;
  /** Reports a used control. Absent: the controls are disabled. */
  onDispatch?: ((handler: string, value: VisualEventValue) => void) | undefined;
  /** An event is on its way to the desktop. */
  pending?: boolean;
  /** Why the last event failed; shown under the tree. */
  error?: string | null;
}

export function VisualTree({ node, onDispatch, pending = false, error = null }: VisualTreeProps) {
  const interactive = controlsOf(node).length > 0;
  return (
    <div data-testid="visual-tree" className="min-w-0 transition-opacity aria-busy:opacity-70" aria-busy={pending || undefined}>
      <ControlsContext value={{ dispatch: onDispatch }}>
        <VisualBoundary>
          <Node node={node} />
        </VisualBoundary>
      </ControlsContext>
      {interactive && !onDispatch ? (
        <p className="pt-2 text-meta text-muted-text" data-testid="visual-hint">
          {OPEN_ON_DESKTOP_HINT}
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="pt-2 text-meta text-deletions" data-testid="visual-error">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/** The most often a slider sends while it is dragged. */
const SLIDER_SEND_MS = 100;
/** An input sends this long after the last keystroke, or at once on Enter or blur. */
const INPUT_IDLE_MS = 800;
const BUTTON_BASE = "inline-flex min-h-11 items-center justify-center rounded-row px-3 text-small pointer-fine:min-h-8";

function useDispatcher(handler: string): ((value: VisualEventValue) => void) | undefined {
  const { dispatch } = use(ControlsContext);
  return dispatch && handler ? (value) => dispatch(handler, value) : undefined;
}

function Button({ node }: { node: Extract<VisualNode, { kind: "button" }> }) {
  const send = useDispatcher(node.handler);
  const disabled = node.disabled || !send;
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={() => send?.(null)}
      className={cn(BUTTON_BASE, "self-start bg-[var(--content-a08)] enabled:hover:bg-[var(--content-a07)] enabled:active:bg-[var(--content-a07)]", disabled && "opacity-40")}
      data-visual="button"
    >
      {node.label}
    </button>
  );
}

function Toggle({ node }: { node: Extract<VisualNode, { kind: "toggle" }> }) {
  const send = useDispatcher(node.handler);
  return (
    <button
      type="button"
      role="switch"
      aria-checked={node.value}
      disabled={!send}
      onClick={() => send?.(!node.value)}
      className={cn("flex min-h-11 w-full items-center justify-between gap-3 text-left text-small pointer-fine:min-h-8", !send && "opacity-60")}
      data-visual="toggle"
    >
      <span className="min-w-0 flex-1">{node.label}</span>
      <span aria-hidden="true" className="inline-flex h-5 w-[34px] shrink-0 items-center rounded-pill p-0.5" style={{ background: node.value ? "var(--accent-color)" : alpha("var(--content)", 15) }}>
        <span className={cn("size-4 rounded-pill bg-inverse", node.value && "ml-3.5")} />
      </span>
    </button>
  );
}

function Tabs({ node }: { node: Extract<VisualNode, { kind: "tabs" }> }) {
  const send = useDispatcher(node.handler);
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const count = node.labels.length;
  // Arrow keys, Home and End move to a tab and select it, as a tablist does (the tab in the tab order is the selected one).
  const go = (to: number) => {
    if (!send || count === 0) return;
    const index = (to + count) % count;
    buttons.current[index]?.focus();
    if (index !== node.selected) send(index);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const keys: Record<string, number> = { ArrowRight: node.selected + 1, ArrowDown: node.selected + 1, ArrowLeft: node.selected - 1, ArrowUp: node.selected - 1, Home: 0, End: count - 1 };
    const to = keys[event.key];
    if (to === undefined) return;
    event.preventDefault();
    go(to);
  };
  return (
    <div role="tablist" onKeyDown={onKeyDown} className="flex flex-wrap gap-1 self-start rounded-row bg-[var(--content-a05)] p-[3px]" data-visual="tabs">
      {node.labels.map((label, i) => (
        <button
          key={`${label}${i}`}
          ref={(element) => {
            buttons.current[i] = element;
          }}
          type="button"
          role="tab"
          aria-selected={i === node.selected}
          tabIndex={i === node.selected ? 0 : -1}
          disabled={!send}
          onClick={() => {
            if (i !== node.selected) send?.(i);
          }}
          className={cn("min-h-11 rounded-[8px] px-3 text-small text-muted-text pointer-fine:min-h-7 pointer-fine:px-2.5", i === node.selected && "bg-[var(--content-a08)] text-content", !send && "opacity-60")}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

function Select({ node }: { node: Extract<VisualNode, { kind: "select" }> }) {
  const send = useDispatcher(node.handler);
  const id = useId();
  return (
    <div className="flex items-center justify-between gap-3 text-small" data-visual="select">
      <label htmlFor={id} className="min-w-0 flex-1">
        {node.label}
      </label>
      <select
        id={id}
        value={node.selected}
        disabled={!send}
        onChange={(event) => send?.(Number(event.target.value))}
        className="min-h-11 max-w-[60%] rounded-row border border-stroke bg-transparent px-2 text-small text-content pointer-fine:min-h-8 disabled:opacity-60"
      >
        {node.options.map((option, i) => (
          <option key={`${option}${i}`} value={i}>
            {option}
          </option>
        ))}
      </select>
    </div>
  );
}

function Slider({ node }: { node: Extract<VisualNode, { kind: "slider" }> }) {
  const send = useDispatcher(node.handler);
  const id = useId();
  // What the thumb shows while it is dragged; the desktop's value takes over with the next tree.
  const [draft, setDraft] = useState<number | null>(null);
  // A drag in progress, or a value not sent yet, keeps the thumb where the user put it: a reply must not pull it back.
  const [holding, setHolding] = useState(false);
  const [unsent, setUnsent] = useState(false);
  const [seen, setSeen] = useState(node);
  if (seen !== node) {
    setSeen(node);
    if (!holding && !unsent) setDraft(null);
  }
  const latest = useRef<number | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const flush = () => {
    clearTimeout(timer.current);
    timer.current = undefined;
    const value = latest.current;
    latest.current = null;
    setUnsent(false);
    if (value !== null) send?.(value);
  };
  const release = () => {
    setHolding(false);
    flush();
  };
  const shown = draft ?? node.value;
  return (
    <div className="flex flex-col gap-1.5" data-visual="slider">
      <div className="flex items-center justify-between gap-3 text-small">
        <label htmlFor={id} className="min-w-0 flex-1">
          {node.label}
        </label>
        <span className="font-mono text-meta text-muted-text">{shown}</span>
      </div>
      <input
        id={id}
        type="range"
        min={node.min}
        max={node.max}
        step={node.step > 0 ? node.step : "any"}
        value={shown}
        disabled={!send}
        onChange={(event) => {
          const value = Number(event.target.value);
          setDraft(value);
          setUnsent(true);
          latest.current = value;
          // While dragging, send at most every SLIDER_SEND_MS (the first move at once, the last one at the end).
          if (timer.current === undefined) {
            flush();
            timer.current = setTimeout(() => {
              timer.current = undefined;
              flush();
            }, SLIDER_SEND_MS);
          }
        }}
        onPointerDown={() => setHolding(true)}
        onPointerUp={release}
        onPointerCancel={release}
        onKeyUp={flush}
        onBlur={release}
        className="min-h-11 w-full accent-[var(--accent-color)] pointer-fine:min-h-6 disabled:opacity-60"
      />
    </div>
  );
}

function TextInput({ node }: { node: Extract<VisualNode, { kind: "input" }> }) {
  const send = useDispatcher(node.handler);
  const id = useId();
  const [draft, setDraft] = useState<string | null>(null);
  // Text typed and not sent yet stays: a reply to an earlier send must not overwrite what the user is typing.
  const [unsent, setUnsent] = useState(false);
  const [seen, setSeen] = useState(node);
  if (seen !== node) {
    setSeen(node);
    if (!unsent) setDraft(null);
  }
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const commit = (text: string) => {
    clearTimeout(timer.current);
    timer.current = undefined;
    setUnsent(false);
    if (text !== node.value) send?.(text);
  };
  const shown = draft ?? node.value;
  return (
    <div className="flex flex-col gap-1.5" data-visual="input">
      <label htmlFor={id} className="text-meta text-muted-text">
        {node.label}
      </label>
      <input
        id={id}
        type="text"
        value={shown}
        placeholder={node.placeholder ?? ""}
        disabled={!send}
        onChange={(event) => {
          const text = event.target.value;
          if (utf8Length(text) > MAX_INPUT_BYTES) return;
          setDraft(text);
          setUnsent(true);
          clearTimeout(timer.current);
          timer.current = setTimeout(() => commit(text), INPUT_IDLE_MS);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit(event.currentTarget.value);
        }}
        onBlur={(event) => commit(event.currentTarget.value)}
        className="min-h-11 rounded-row border border-stroke bg-transparent px-2.5 text-small text-content pointer-fine:min-h-8 disabled:opacity-60"
      />
    </div>
  );
}

function Node({ node }: { node: VisualNode }) {
  const color = useVisualColor();
  switch (node.kind) {
    case "column":
    case "card":
      return (
        <div className={cn("flex flex-col", node.kind === "card" && "rounded-card border border-stroke bg-[var(--content-a04)] p-4")} style={{ gap: node.gap }}>
          {node.children.map((child, i) => (
            <Node key={i} node={child} />
          ))}
        </div>
      );
    case "row":
      return (
        <div className="flex flex-wrap items-start" style={{ gap: node.gap }}>
          {node.children.map((child, i) => (
            <div key={i} className="min-w-[120px] flex-1">
              <Node node={child} />
            </div>
          ))}
        </div>
      );
    case "grid":
      return (
        <div className="grid" style={{ gap: node.gap, gridTemplateColumns: `repeat(auto-fit, minmax(min(100%, 180px), 1fr))` }} data-columns={node.columns}>
          {node.children.map((child, i) => (
            <div key={i} className="min-w-0">
              <Node node={child} />
            </div>
          ))}
        </div>
      );
    case "text":
      return (
        <p className={cn("wrap-anywhere", TEXT_CLASS[node.size] ?? "text-body")} style={{ color: color(node.color ?? "ink") }}>
          {node.text}
        </p>
      );
    case "stat": {
      let delta = node.delta ?? null;
      let detail = node.detail ?? null;
      // "+12.4% vs last month" lends its leading change to the delta.
      if (!delta && detail) {
        const match = /^([+\-−]\d\S*)\s*(.*)$/.exec(detail);
        if (match) {
          delta = match[1] ?? null;
          detail = match[2] || null;
        }
      }
      const tint = ["ink", "muted", "faint"].includes(node.color) ? "var(--accent-color)" : color(node.color);
      const down = !!delta && (delta.startsWith("-") || delta.startsWith("−"));
      return (
        <div className="flex flex-col gap-1">
          <span className="truncate text-small text-muted-text">{node.label}</span>
          <span className="text-[30px] leading-[38px] font-semibold tabular-nums" style={{ color: color(node.color ?? "ink") }}>
            {node.value}
          </span>
          {delta || detail ? (
            <span className="flex flex-wrap items-center gap-x-2 text-meta text-muted-text">
              {delta ? (
                <span className="font-mono" style={{ color: color(down !== !!node.lower_is_better ? "negative" : "positive") }}>
                  {down ? "▼" : "▲"} {delta}
                </span>
              ) : null}
              {detail ? <span>{detail}</span> : null}
            </span>
          ) : null}
          {node.trend && node.trend.length > 1 ? (
            <div className="pt-2">
              <Trend values={node.trend} tint={tint} />
            </div>
          ) : null}
        </div>
      );
    }
    case "table":
      return (
        <div className="overflow-x-auto">
          <table className="border-collapse text-small">
            <thead>
              <tr>
                {node.columns.map((column, c) => (
                  <th key={c} scope="col" className="min-w-[110px] border-b border-stroke px-2 py-2 text-left font-semibold text-muted-text">
                    {column}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {node.rows.map((cells, r) => (
                <tr key={r}>
                  {cells.map((cell, c) => (
                    <td key={c} className="min-w-[110px] border-b border-stroke px-2 py-2">
                      {cell}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "chart":
      return <Cartesian kind={node.chart_type} series={node.series} labels={node.labels} height={node.height || 220} />;
    case "gauge":
      return <Gauge label={node.label} value={node.value} min={node.min} max={node.max} unit={node.unit ?? null} tint={color(node.color)} />;
    case "donut":
      return <Donut slices={node.slices} label={node.label ?? null} value={node.value ?? null} />;
    case "heatmap":
      return <Heatmap values={node.values} rows={node.rows} columns={node.columns} tint={color(node.color)} />;
    case "progress":
      return <Progress label={node.label} value={node.value} max={node.max} detail={node.detail ?? null} tint={color(node.color)} />;
    case "button":
      return <Button node={node} />;
    case "toggle":
      return <Toggle node={node} />;
    case "input":
      return <TextInput node={node} />;
    case "select":
      return <Select node={node} />;
    case "slider":
      return <Slider node={node} />;
    case "tabs":
      return <Tabs node={node} />;
    case "canvas":
      return (
        <svg
          viewBox={`0 0 ${node.width} ${node.height}`}
          role="img"
          aria-label="Canvas"
          preserveAspectRatio="xMidYMin meet"
          className="block h-auto w-full"
          style={{ maxHeight: node.height }}
          data-visual="canvas"
        >
          <CanvasShapes shapes={node.shapes} />
        </svg>
      );
    case "flow":
      return <LazyFlowDiagram graph={node.graph} title={node.title ?? "Diagram"} ariaLabel={`${node.title ?? "Flow"} diagram`} boardHeight={node.height} testId="visual-flow" />;
    case "scene":
      return (
        <div className="grid place-items-center rounded-card border border-stroke bg-[var(--content-a04)] p-4 text-small text-muted-text" style={{ aspectRatio: node.width / node.height || 16 / 9 }}>
          Open this 3D scene on your desktop to see it.
        </div>
      );
    default:
      return null;
  }
}
