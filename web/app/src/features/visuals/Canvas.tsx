// A `visual.canvas` drawn as one inline SVG, as the desktop's `drawing` in crates/monocode-gpui/src/visuals.rs: rects
// with radius and stroke, circles, lines and polylines with arrowheads and dashes, and labels anchored by align and
// baseline, optionally on a chip, with automatic dark or light ink. Data only; label text is a text child.
import { useEffect, useMemo, useState } from "react";
import { arrowhead } from "@/features/diagram/layout";
import { ASCENT, LABEL_FONT, labelOrigin } from "@/features/diagram/model";
import { useAppearance } from "@/lib/appearance";
import { alpha, useVisualColor } from "./charts";
import { type Rgb, accentRgb, inkOn, labelBackdrop, tokenRgb } from "./ink";
import type { ArrowEnds, Shape } from "./types";

const FONT_FAMILY = "ui-sans-serif, system-ui, sans-serif";
const SVG_ANCHOR = { start: "start", center: "middle", end: "end" } as const;

let context: CanvasRenderingContext2D | null | undefined;
let measurer: ((text: string, font: number) => number) | null = null;
const widths = new Map<string, number>();

/** Replaces the text measurer (tests); null restores the browser's canvas measurement. */
export function setLabelMeasurer(next: ((text: string, font: number) => number) | null): void {
  measurer = next;
  widths.clear();
}

/** The advance of `text` in the label font: measured when the browser can, else a plain estimate. Cached per font and text. */
export function measureLabel(text: string, font: number): number {
  const key = `${font}|${text}`;
  const known = widths.get(key);
  if (known !== undefined) return known;
  let width: number;
  if (measurer) {
    width = measurer(text, font);
  } else {
    if (context === undefined) {
      try {
        context = typeof navigator !== "undefined" && /jsdom/i.test(navigator.userAgent) ? null : document.createElement("canvas").getContext("2d");
      } catch {
        context = null;
      }
    }
    if (context) {
      context.font = `${font}px ${FONT_FAMILY}`;
      width = context.measureText(text).width;
    } else {
      width = [...text].length * font * 0.56;
    }
  }
  if (widths.size > 512) widths.clear();
  widths.set(key, width);
  return width;
}

/** Bumps when the page's web fonts finish loading, after which cached widths are stale and chips are measured again. */
function useFontsVersion(): number {
  const [version, setVersion] = useState(0);
  useEffect(() => {
    const fonts = typeof document !== "undefined" ? document.fonts : undefined;
    if (!fonts) return undefined;
    let live = true;
    const refresh = () => {
      if (!live) return;
      widths.clear();
      setVersion((v) => v + 1);
    };
    void fonts.ready.then(refresh, () => {});
    fonts.addEventListener?.("loadingdone", refresh);
    return () => {
      live = false;
      fonts.removeEventListener?.("loadingdone", refresh);
    };
  }, []);
  return version;
}

const pointList = (points: readonly (readonly [number, number])[]) => points.map((p) => `${p[0]},${p[1]}`).join(" ");

function Heads({ points, ends, width, ink }: { points: [number, number][]; ends: ArrowEnds; width: number; ink: string }) {
  const n = points.length;
  if (n < 2) return null;
  const size = Math.max(width * 4, 8);
  return (
    <>
      {ends === "end" || ends === "both" ? <polygon data-arrow="end" points={pointList(arrowhead(points[n - 1]!, points[n - 2]!, size))} fill={ink} /> : null}
      {ends === "start" || ends === "both" ? <polygon data-arrow="start" points={pointList(arrowhead(points[0]!, points[1]!, size))} fill={ink} /> : null}
    </>
  );
}

export function CanvasShapes({ shapes }: { shapes: readonly Shape[] }) {
  const color = useVisualColor();
  const scheme = useAppearance((state) => state.scheme);
  const accent = useMemo<Rgb>(() => accentRgb(scheme), [scheme]);
  useFontsVersion();
  return (
    <>
      {shapes.map((shape, index) => {
        switch (shape.kind) {
          case "rect": {
            const radius = Math.min(shape.radius, shape.width / 2, shape.height / 2);
            return (
              <rect
                key={index}
                x={shape.x}
                y={shape.y}
                width={Math.max(shape.width, 0)}
                height={Math.max(shape.height, 0)}
                rx={Math.max(radius, 0)}
                fill={shape.fill ? color(shape.color) : "none"}
                {...(shape.stroke ? { stroke: color(shape.stroke), strokeWidth: shape.stroke_width } : {})}
              />
            );
          }
          case "circle":
            return <circle key={index} cx={shape.x} cy={shape.y} r={Math.max(shape.radius, 0)} fill={shape.fill ? color(shape.color) : "none"} {...(shape.stroke ? { stroke: color(shape.stroke), strokeWidth: shape.stroke_width } : {})} />;
          case "line": {
            const points: [number, number][] = [[shape.x1, shape.y1], [shape.x2, shape.y2]];
            const ink = color(shape.color);
            return (
              <g key={index}>
                <line x1={shape.x1} y1={shape.y1} x2={shape.x2} y2={shape.y2} stroke={ink} strokeWidth={shape.width} strokeDasharray={shape.dashed ? `${6 * Math.max(shape.width, 1)} ${4 * Math.max(shape.width, 1)}` : undefined} />
                <Heads points={points} ends={shape.arrow} width={shape.width} ink={ink} />
              </g>
            );
          }
          case "polyline": {
            const ink = color(shape.color);
            return (
              <g key={index}>
                <polyline points={pointList(shape.points)} stroke={ink} strokeWidth={shape.width} fill="none" strokeLinejoin="round" strokeDasharray={shape.dashed ? `${6 * Math.max(shape.width, 1)} ${4 * Math.max(shape.width, 1)}` : undefined} />
                <Heads points={shape.points} ends={shape.arrow} width={shape.width} ink={ink} />
              </g>
            );
          }
          case "label": {
            const backdrop = shape.color ? null : labelBackdrop(shapes, index);
            const ink = shape.color ? color(shape.color) : backdrop ? inkOn(tokenRgb(backdrop, scheme, accent)) : "var(--content)";
            const font = LABEL_FONT;
            const width = shape.chip ? measureLabel(shape.text, font) : 0;
            const [left, top] = labelOrigin(shape.x, shape.y, width, font, shape.align, shape.baseline);
            const pad = font * 0.35;
            return (
              <g key={index} data-label-align={shape.align} data-label-baseline={shape.baseline}>
                {shape.chip ? <rect x={left - pad} y={top - font * 0.1} width={width + 2 * pad} height={font * 1.6} rx={font * 0.3} fill="var(--surface-raised)" stroke={alpha("var(--content)", 20)} strokeWidth={1} data-chip="true" /> : null}
                <text x={shape.x} y={top + ASCENT * font} textAnchor={SVG_ANCHOR[shape.align]} fill={ink} fontSize={font} fontFamily={FONT_FAMILY}>
                  {shape.text}
                </text>
              </g>
            );
          }
        }
      })}
    </>
  );
}
