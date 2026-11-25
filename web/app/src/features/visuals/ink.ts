// Automatic label ink on a canvas: dark or light by the luminance of the filled shape under the label's anchor, as the
// desktop does (`label_backdrop` in crates/monocode-core/src/visual/model.rs and `ink_on` in monocode-gpui visuals.rs).
import { luminance, prefersDarkText } from "@/features/diagram/model";
import type { Shape, ThemeColor } from "./types";

export type Rgb = [number, number, number];
export type Scheme = "dark" | "light";

/** The fill under a label: the last earlier filled rect or circle that contains the label's anchor point. */
export function labelBackdrop(shapes: readonly Shape[], index: number): ThemeColor | null {
  const label = shapes[index];
  if (!label || label.kind !== "label") return null;
  for (let i = index - 1; i >= 0; i--) {
    const shape = shapes[i]!;
    if (shape.kind === "rect" && shape.fill && label.x >= shape.x && label.x <= shape.x + shape.width && label.y >= shape.y && label.y <= shape.y + shape.height) return shape.color;
    if (shape.kind === "circle" && shape.fill && (label.x - shape.x) ** 2 + (label.y - shape.y) ** 2 <= shape.radius * shape.radius) return shape.color;
  }
  return null;
}

const hex = (value: number): Rgb => [(value >> 16) & 255, (value >> 8) & 255, value & 255];

// The desktop's series hues (tokens.css --viz-*), dark then light surfaces.
const HUES: Partial<Record<ThemeColor, [number, number]>> = {
  positive: [0x4fd57f, 0x0a924b],
  negative: [0xfc5855, 0xd01d21],
  warning: [0xfea247, 0xd96700],
  series2: [0x4fd57f, 0x0a924b],
  series3: [0xb77df8, 0x863ad2],
  series4: [0xfea247, 0xd96700],
  series5: [0xfa78b2, 0xd12c80],
  series6: [0x3faff3, 0x0077d7],
};
const SURFACE: Record<Scheme, Rgb> = { dark: hex(0x2a2a2a), light: hex(0xe3e3e3) };
const CONTENT: Record<Scheme, Rgb> = { dark: hex(0xebebeb), light: hex(0x2e2e2e) };
const ACCENT: Record<Scheme, Rgb> = { dark: hex(0x4c9ef5), light: hex(0x1f7ae0) };

const mix = (top: Rgb, alpha: number, base: Rgb): Rgb => [0, 1, 2].map((i) => top[i]! * alpha + base[i]! * (1 - alpha)) as Rgb;

/** The accent colour as the page resolves it now (it follows the user's accent), or the scheme default. */
export function accentRgb(scheme: Scheme): Rgb {
  try {
    const raw = getComputedStyle(document.documentElement).getPropertyValue("--accent-color").trim();
    const match = /^#([0-9a-f]{6})$/i.exec(raw);
    if (match) return hex(parseInt(match[1]!, 16));
  } catch {
    // no layout engine: the default below
  }
  return ACCENT[scheme];
}

/** A theme colour as an opaque sRGB colour over the surface, for the contrast decision only. */
export function tokenRgb(token: ThemeColor, scheme: Scheme, accent: Rgb = accentRgb(scheme)): Rgb {
  const surface = SURFACE[scheme];
  if (token === "ink") return CONTENT[scheme];
  if (token === "muted") return mix(CONTENT[scheme], 0.6, surface);
  if (token === "faint") return mix(CONTENT[scheme], 0.2, surface);
  const hue = HUES[token];
  return hue ? hex(hue[scheme === "dark" ? 0 : 1]) : accent;
}

export const INK_DARK = "#15171a";
export const INK_LIGHT = "#f5f5f7";

/** Dark or light ink for text on a fill of this colour. */
export const inkOn = (fill: Rgb): string => (prefersDarkText(luminance([fill[0] / 255, fill[1] / 255, fill[2] / 255])) ? INK_DARK : INK_LIGHT);
