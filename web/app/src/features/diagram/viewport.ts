// Pan and zoom math of the diagram viewer, ported from crates/monocode-core/src/diagram/viewport.rs (same constants,
// same formulas). `x` and `y` are the screen offset of the content origin; a content point `p` is shown at
// `p * scale + offset`. Every function is pure and returns a new viewport.
export const MIN_SCALE = 0.25;
export const MAX_SCALE = 4;
/** Fit never enlarges beyond this, so a tiny diagram is not blown up. */
export const FIT_MAX = 1.6;
export const FIT_MARGIN = 20;
export const BUTTON_STEP = 1.2;
export const DOT_SPACING = 18;

export type Viewport = { scale: number; x: number; y: number };

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

export const DEFAULT_VIEWPORT: Viewport = { scale: 1, x: 0, y: 0 };

/** Centre `content` in `board` at the largest scale that fits with a margin. */
export function fitViewport(content: [number, number], board: [number, number]): Viewport {
  const cw = Math.max(content[0], 1);
  const ch = Math.max(content[1], 1);
  const scale = clamp(Math.min((board[0] - 2 * FIT_MARGIN) / cw, (board[1] - 2 * FIT_MARGIN) / ch, FIT_MAX), MIN_SCALE, MAX_SCALE);
  return { scale, x: (board[0] - cw * scale) / 2, y: (board[1] - ch * scale) / 2 };
}

/** Multiply the scale by `factor` keeping the content point under the screen position `(px, py)` fixed. */
export function zoomAt(view: Viewport, factor: number, px: number, py: number): Viewport {
  const next = clamp(view.scale * factor, MIN_SCALE, MAX_SCALE);
  return { scale: next, x: px - ((px - view.x) * next) / view.scale, y: py - ((py - view.y) * next) / view.scale };
}

export const panBy = (view: Viewport, dx: number, dy: number): Viewport => ({ ...view, x: view.x + dx, y: view.y + dy });

export const percent = (view: Viewport): number => Math.round(view.scale * 100);

/** The content point shown at a screen position. */
export const toContent = (view: Viewport, px: number, py: number): [number, number] => [(px - view.x) / view.scale, (py - view.y) / view.scale];

export const toScreen = (view: Viewport, cx: number, cy: number): [number, number] => [cx * view.scale + view.x, cy * view.scale + view.y];

/** Zoom factor for a wheel scroll of `deltaY` pixels (positive scrolls down, which zooms out). One 20px notch is about 10%. */
export const wheelFactor = (deltaY: number): number => clamp(Math.exp(-deltaY * 0.005), 0.5, 2);

/** Zoom factor for a pinch delta (0.1 means 10% in). */
export const pinchFactor = (delta: number): number => clamp(1 + delta, 0.5, 2);

/** The dot spacing of the board at a scale: doubled while the dots would crowd (desktop `paint_dots`). */
export function dotSpacing(scale: number): number {
  let spacing = DOT_SPACING * scale;
  while (spacing < 9) spacing *= 2;
  return spacing;
}

/** The height of the board outside fullscreen for content of this size. */
export const MIN_BOARD = 180;
export const MAX_BOARD = 420;
export const boardHeight = (contentHeight: number, override?: number | null): number => clamp(override ?? contentHeight + 2 * FIT_MARGIN, MIN_BOARD, MAX_BOARD);
