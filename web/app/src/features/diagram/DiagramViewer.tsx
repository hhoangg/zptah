// The one diagram viewer of the web app: Mermaid blocks and `visual.flow` both render here (the desktop twin is
// crates/monocode-gpui/src/diagram_viewer.rs). A dotted board that pans and zooms with its content, a toolbar (zoom
// out, zoom %, zoom in, Fit, Fullscreen, Source, Copy), drag to pan, pinch and ctrl/cmd + wheel zoom at the pointer
// (a plain wheel scrolls the page), double click or double tap to fit, fit on first render and refit on resize while
// the view is untouched. The content is a child SVG in content coordinates; all pan and zoom math is viewport.ts.
import { type CSSProperties, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { safeClipboardText } from "@/lib/clipboardText";
import { cn } from "@/lib/utils";
import { alpha } from "@/features/visuals/charts";
import { CodeBlock } from "@/features/markdown/CodeBlock";
import { BUTTON_STEP, DEFAULT_VIEWPORT, type Viewport, boardHeight, dotSpacing, fitViewport, panBy, percent, pinchFactor, wheelFactor, zoomAt } from "./viewport";

export interface DiagramViewerProps {
  title: string;
  /** Names the board for assistive technology. */
  ariaLabel: string;
  /** The size of the content in content units. */
  width: number;
  height: number;
  /** Board height outside fullscreen; absent sizes it from the content. */
  boardHeight?: number | null;
  /** Enables the Source toggle and the Copy button. */
  source?: string;
  sourceLanguage?: string;
  /** A note shown above the board. */
  status?: string;
  /** The drawing, in content coordinates, as SVG children. */
  children: ReactNode;
  testId?: string;
}

const TAP_MS = 350;
const TAP_SLOP = 24;
const DRAG_SLOP = 6;

type Tap = { at: number; x: number; y: number };

function toolButton(className?: string) {
  return cn(
    "inline-flex h-9 min-w-9 shrink-0 items-center justify-center rounded-[8px] px-2 text-caption text-[color:var(--content-a60)] hover:bg-[var(--content-a10)] hover:text-content pointer-fine:h-7 pointer-fine:min-w-7",
    className,
  );
}

function pageScale(event: WheelEvent, board: HTMLElement): [number, number] {
  // Lines and pages become pixels as the desktop does (a line is 20 px).
  const unit = event.deltaMode === 1 ? 20 : event.deltaMode === 2 ? board.clientHeight : 1;
  return [event.deltaX * unit, event.deltaY * unit];
}

export function DiagramViewer(props: DiagramViewerProps) {
  const { title, ariaLabel, width, height, source, children, testId } = props;
  const [view, setView] = useState<Viewport>(DEFAULT_VIEWPORT);
  const [showSource, setShowSource] = useState(false);
  const [copied, setCopied] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  const [dragging, setDragging] = useState(false);
  const boardSize = useRef<[number, number]>([0, 0]);
  const boardEl = useRef<HTMLDivElement | null>(null);
  const fitted = useRef(false);
  const touched = useRef(false);
  const fullscreenRef = useRef(false);
  const contentRef = useRef<[number, number]>([width, height]);
  const copyTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const fullButton = useRef<HTMLButtonElement | null>(null);
  const [placeholder, setPlaceholder] = useState(0);
  const hasSource = source !== undefined;
  const sourceShown = hasSource && showSource;

  useLayoutEffect(() => {
    fullscreenRef.current = fullscreen;
  }, [fullscreen]);

  const fit = useCallback(() => {
    const [w, h] = boardSize.current;
    if (w <= 1 || h <= 1) return;
    setView(fitViewport(contentRef.current, boardSize.current));
    fitted.current = true;
    touched.current = false;
  }, []);

  // New content (another graph) is fitted again.
  useLayoutEffect(() => {
    contentRef.current = [width, height];
    fitted.current = false;
    fit();
  }, [width, height, fit]);

  // The board element comes and goes (source view, fullscreen portal): measure it when it appears and on resize.
  const boardRef = useCallback(
    (el: HTMLDivElement | null) => {
      boardEl.current = el;
      if (!el) return undefined;
      const measure = () => {
        const rect = el.getBoundingClientRect();
        const next: [number, number] = [rect.width, rect.height];
        const resized = next[0] !== boardSize.current[0] || next[1] !== boardSize.current[1];
        boardSize.current = next;
        // Keep a user's own pan and zoom on resize; refit only the untouched view.
        if (!fitted.current || (resized && !touched.current)) fit();
      };
      measure();
      const observer = typeof ResizeObserver === "function" ? new ResizeObserver(measure) : null;
      observer?.observe(el);
      const onWheel = (event: WheelEvent) => {
        const [dx, dy] = pageScale(event, el);
        const secondary = event.ctrlKey || event.metaKey;
        const zoom = (fullscreenRef.current || secondary) && dy !== 0;
        // A plain vertical scroll belongs to the page (the chat); a mostly horizontal one pans.
        if (!zoom && (dx === 0 || (!fullscreenRef.current && !secondary && Math.abs(dx) <= Math.abs(dy)))) return;
        event.preventDefault();
        touched.current = true;
        if (zoom) {
          const rect = el.getBoundingClientRect();
          const x = event.clientX - rect.left;
          const y = event.clientY - rect.top;
          setView((v) => zoomAt(v, wheelFactor(dy), x, y));
        } else {
          setView((v) => panBy(v, -dx, 0));
        }
      };
      el.addEventListener("wheel", onWheel, { passive: false });
      return () => {
        observer?.disconnect();
        el.removeEventListener("wheel", onWheel);
      };
    },
    [fit],
  );

  const exitFullscreen = useCallback(() => setFullscreen(false), []);

  // Esc leaves fullscreen. The panel handles it first (below); this fallback covers focus outside the panel, unless
  // the key already was handled or focus sits in another dialog or menu, which owns its own Esc.
  useEffect(() => {
    if (!fullscreen) return undefined;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      const owner = document.activeElement?.closest("[role='dialog'], [role='alertdialog'], [role='menu'], [role='listbox']");
      if (owner && owner !== panelRef.current) return;
      event.preventDefault();
      exitFullscreen();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [fullscreen, exitFullscreen]);

  // A modal: the page behind does not scroll, focus stays inside, and it returns to the Full button on exit.
  useEffect(() => {
    if (!fullscreen) return undefined;
    const previous = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    const onFocusIn = (event: FocusEvent) => {
      const panel = panelRef.current;
      if (panel && event.target instanceof Node && !panel.contains(event.target)) panel.focus();
    };
    document.addEventListener("focusin", onFocusIn);
    return () => {
      document.body.style.overflow = previous;
      document.removeEventListener("focusin", onFocusIn);
    };
  }, [fullscreen]);

  const wasFullscreen = useRef(false);
  useEffect(() => {
    if (fullscreen) panelRef.current?.focus();
    else if (wasFullscreen.current) fullButton.current?.focus();
    wasFullscreen.current = fullscreen;
  }, [fullscreen]);

  useEffect(() => () => clearTimeout(copyTimer.current), []);

  const zoomCentre = (factor: number) => {
    const [w, h] = boardSize.current;
    touched.current = true;
    setView((v) => zoomAt(v, factor, w / 2, h / 2));
  };

  const toggleFullscreen = () => {
    // The inline panel is replaced by a placeholder of its own height, so the transcript does not jump.
    if (!fullscreen) setPlaceholder(panelRef.current?.getBoundingClientRect().height ?? 0);
    fitted.current = false;
    touched.current = false;
    setFullscreen(!fullscreen);
  };

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(safeClipboardText(source ?? ""));
    } catch {
      return;
    }
    setCopied(true);
    clearTimeout(copyTimer.current);
    copyTimer.current = setTimeout(() => setCopied(false), 1500);
  };

  // Pointers: one pans, two pinch (zoom at their midpoint while it also pans), a quick second tap fits.
  const pointers = useRef(new Map<number, [number, number]>());
  const gesture = useRef<{ start: [number, number]; at: number; moved: boolean }>({ start: [0, 0], at: 0, moved: false });
  const lastTap = useRef<Tap | null>(null);
  const local = (event: ReactPointerEvent): [number, number] => {
    const rect = boardEl.current?.getBoundingClientRect();
    return [event.clientX - (rect?.left ?? 0), event.clientY - (rect?.top ?? 0)];
  };
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.pointerType === "mouse" && event.button !== 0) return;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    const at = local(event);
    if (pointers.current.size === 0) gesture.current = { start: at, at: event.timeStamp, moved: false };
    else gesture.current.moved = true;
    pointers.current.set(event.pointerId, at);
    setDragging(true);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const previous = pointers.current.get(event.pointerId);
    if (!previous) return;
    const next = local(event);
    const others = [...pointers.current.entries()].filter(([id]) => id !== event.pointerId);
    touched.current = true;
    if (others.length === 0) {
      if (Math.hypot(next[0] - gesture.current.start[0], next[1] - gesture.current.start[1]) > DRAG_SLOP) gesture.current.moved = true;
      pointers.current.set(event.pointerId, next);
      setView((v) => panBy(v, next[0] - previous[0], next[1] - previous[1]));
      return;
    }
    const other = others[0]![1];
    const before = Math.hypot(previous[0] - other[0], previous[1] - other[1]);
    const after = Math.hypot(next[0] - other[0], next[1] - other[1]);
    const midBefore = [(previous[0] + other[0]) / 2, (previous[1] + other[1]) / 2] as const;
    const midAfter = [(next[0] + other[0]) / 2, (next[1] + other[1]) / 2] as const;
    pointers.current.set(event.pointerId, next);
    gesture.current.moved = true;
    const factor = before > 0 ? pinchFactor(after / before - 1) : 1;
    setView((v) => zoomAt(panBy(v, midAfter[0] - midBefore[0], midAfter[1] - midBefore[1]), factor, midAfter[0], midAfter[1]));
  };
  const onPointerEnd = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!pointers.current.has(event.pointerId)) return;
    const at = pointers.current.get(event.pointerId)!;
    pointers.current.delete(event.pointerId);
    if (pointers.current.size === 0) setDragging(false);
    if (event.type === "pointercancel" || gesture.current.moved || pointers.current.size > 0) {
      lastTap.current = null;
      return;
    }
    const previous = lastTap.current;
    if (previous && event.timeStamp - previous.at < TAP_MS && Math.hypot(at[0] - previous.x, at[1] - previous.y) < TAP_SLOP) {
      lastTap.current = null;
      fit();
    } else {
      lastTap.current = { at: event.timeStamp, x: at[0], y: at[1] };
    }
  };
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const step = 40;
    let handled = true;
    if (event.key === "+" || event.key === "=") zoomCentre(BUTTON_STEP);
    else if (event.key === "-" || event.key === "_") zoomCentre(1 / BUTTON_STEP);
    else if (event.key === "0") fit();
    else if (event.key === "ArrowLeft") setView((v) => panBy(v, step, 0));
    else if (event.key === "ArrowRight") setView((v) => panBy(v, -step, 0));
    else if (event.key === "ArrowUp") setView((v) => panBy(v, 0, step));
    else if (event.key === "ArrowDown") setView((v) => panBy(v, 0, -step));
    else handled = false;
    if (!handled) return;
    touched.current = true;
    event.preventDefault();
    event.stopPropagation();
  };

  const spacing = dotSpacing(view.scale);
  const boardStyle: CSSProperties = {
    backgroundColor: alpha("var(--content)", 3.5),
    backgroundImage: `radial-gradient(${alpha("var(--content)", 20)} 1.2px, transparent 1.3px)`,
    backgroundSize: `${spacing}px ${spacing}px`,
    backgroundPosition: `${view.x - spacing / 2}px ${view.y - spacing / 2}px`,
    ...(fullscreen ? {} : { height: boardHeight(height, props.boardHeight) }),
  };
  const contentStyle: CSSProperties = {
    position: "absolute",
    left: 0,
    top: 0,
    transformOrigin: "0 0",
    transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
    overflow: "visible",
  };
  const coarse = typeof matchMedia === "function" && matchMedia("(pointer: coarse)").matches;

  const toolbar = (
    <div className="flex flex-wrap items-center gap-1 px-2.5 py-1.5 text-caption text-[color:var(--content-a65)]">
      <div className="min-w-0 flex-1 basis-24 truncate font-medium text-[color:var(--content-a80)]" data-testid="diagram-title">
        {title}
      </div>
      <button type="button" className={toolButton()} aria-label="Zoom out" onClick={() => zoomCentre(1 / BUTTON_STEP)}>
        &minus;
      </button>
      <span className="min-w-[46px] text-center text-caption text-[color:var(--content-a60)] tabular-nums" data-testid="diagram-zoom" aria-live="off">
        {percent(view)}%
      </span>
      <button type="button" className={toolButton()} aria-label="Zoom in" onClick={() => zoomCentre(BUTTON_STEP)}>
        +
      </button>
      <button type="button" className={toolButton()} aria-label="Fit to view" onClick={fit}>
        Fit
      </button>
      <button ref={fullButton} type="button" className={toolButton()} aria-label={fullscreen ? "Exit fullscreen" : "Fullscreen"} aria-pressed={fullscreen} onClick={toggleFullscreen}>
        {fullscreen ? "Exit" : "Full"}
      </button>
      {hasSource ? (
        <>
          <button type="button" className={toolButton()} aria-label={sourceShown ? "Show diagram" : "Show source"} aria-pressed={sourceShown} onClick={() => setShowSource(!sourceShown)}>
            {sourceShown ? "Diagram" : "Source"}
          </button>
          <button type="button" className={toolButton()} aria-label={copied ? "Copied" : "Copy code"} onClick={() => void copy()}>
            {copied ? "Copied" : "Copy"}
          </button>
        </>
      ) : null}
    </div>
  );

  const body = sourceShown ? (
    <div className={cn("overflow-auto", fullscreen && "min-h-0 flex-1")} style={fullscreen ? undefined : { height: boardHeight(height, props.boardHeight) }} data-testid="diagram-source">
      <CodeBlock code={source ?? ""} language={props.sourceLanguage ?? ""} />
    </div>
  ) : (
    <div
      ref={boardRef}
      role="img"
      aria-label={ariaLabel}
      tabIndex={0}
      data-testid="diagram-board"
      data-dragging={dragging || undefined}
      className={cn("relative touch-none overflow-hidden outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent-color)] focus-visible:ring-inset", dragging ? "cursor-grabbing" : "cursor-grab", fullscreen && "min-h-0 flex-1")}
      style={boardStyle}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerEnd}
      onPointerCancel={onPointerEnd}
      onKeyDown={onKeyDown}
    >
      <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden="true" focusable="false" style={contentStyle} data-testid="diagram-content">
        {children}
      </svg>
      <div className="pointer-events-none absolute right-2.5 bottom-2 text-[11px] text-[color:var(--content-a40)]">
        {coarse ? "Drag to pan, pinch to zoom, double tap to fit" : "Drag to pan, ctrl or cmd + scroll to zoom, double click to fit"}
      </div>
    </div>
  );

  const onPanelKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!fullscreen) return;
    if (event.key === "Escape" && !event.defaultPrevented) {
      event.preventDefault();
      event.stopPropagation();
      exitFullscreen();
      return;
    }
    if (event.key !== "Tab") return;
    const panel = panelRef.current;
    if (!panel) return;
    const items = [...panel.querySelectorAll<HTMLElement>("button:not([disabled]), [href], input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex='-1'])")];
    const first = items[0];
    const last = items.at(-1);
    if (!first || !last) return;
    const active = document.activeElement;
    if (event.shiftKey && (active === first || active === panel)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  };

  const panel = (
    <div
      ref={panelRef}
      onKeyDown={onPanelKeyDown}
      tabIndex={-1}
      role={fullscreen ? "dialog" : "group"}
      aria-label={title}
      aria-modal={fullscreen || undefined}
      data-testid={testId ?? "diagram-viewer"}
      data-fullscreen={fullscreen || undefined}
      className={cn("flex min-w-0 flex-col overflow-hidden rounded-card border border-stroke outline-none", fullscreen ? "h-full w-full bg-surface-raised" : "bg-[var(--content-a04)]")}
    >
      {toolbar}
      {props.status ? <div className="truncate px-3 pb-1 text-caption text-[color:var(--content-a50)]">{props.status}</div> : null}
      <div className={cn("flex min-w-0 flex-col border-t border-stroke", fullscreen && "min-h-0 flex-1")}>{body}</div>
    </div>
  );

  if (!fullscreen) return panel;
  // A fixed overlay inside a transformed ancestor (the virtualised transcript) would be clipped: portal it to the body.
  return (
    <>
      <div aria-hidden="true" data-testid="diagram-placeholder" style={{ height: placeholder }} />
      {createPortal(
        <div className="fixed inset-0 z-[200] flex bg-black/55 p-0 sm:p-4" data-testid="diagram-fullscreen">
          {panel}
        </div>,
        document.body,
      )}
    </>
  );
}
