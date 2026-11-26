// Draws Mermaid source to a standalone SVG string with the official `mermaid` library (every diagram type it supports).
// This module is only reached through a dynamic import from MermaidBlock, and `mermaid` itself is loaded with its own
// dynamic import on the first call, so a transcript without a diagram never downloads either.
//
// The output never goes into the page as markup: the caller turns the string into a `blob:` image (see MermaidBlock), which
// runs no script, loads nothing external and whose own <style> is not governed by the page CSP. Rendering still builds a
// scratch SVG in the live document (mermaid measures text with getBBox) and sanitizes it with DOMPurify, which needs the
// `dompurify` Trusted Types policy of security.md 8.2. Mermaid's own `innerHTML` writes (theme CSS, icons, math, HTML labels)
// are handled by the capture window of trusted-types.ts, with the sanitizer below. Everywhere else the `default` policy refuses.
import type Mermaid from "mermaid";
import type { Config as PurifyConfig } from "dompurify";
import { captureMermaidStyle, releaseMermaidCapture } from "@/trusted-types";
import type { Scheme } from "@/styles/themes";

export type MermaidRender = { ok: true; svg: string; width: number; height: number } | { ok: false; error: string };

type MermaidApi = typeof Mermaid;

type Purify = (html: string) => string;
type Loaded = { api: MermaidApi; purify: Purify };

/** Longest a single diagram may take (load, parse, layout) before it shows as source; the queue moves on. */
export const RENDER_TIMEOUT_MS = 15_000;

/**
 * What a `.html()` write of mermaid may contain: the HTML, SVG and SVG filter profiles, and no active or external content.
 * (DOMPurify already drops event handler attributes and `javascript:` URLs; the lists below are belt and braces.)
 */
const PURIFY_CONFIG: PurifyConfig = {
  USE_PROFILES: { html: true, svg: true, svgFilters: true },
  FORBID_TAGS: ["script", "style", "iframe", "frame", "object", "embed", "link", "meta", "base", "form", "input", "button", "textarea", "select", "foreignObject", "animate", "set"],
  FORBID_ATTR: ["srcdoc", "formaction", "onload", "onerror", "onclick", "onmouseover", "onfocus"],
  ALLOW_UNKNOWN_PROTOCOLS: false,
  RETURN_TRUSTED_TYPE: false,
};

/** The part of DOMPurify the sanitizer uses (the module is only loaded lazily, so it is taken as a parameter). */
interface PurifyApi {
  sanitize(html: string, config: PurifyConfig): string | { toString(): string };
}

const SVG_FRAGMENT = /^\s*<(?:g|svg)[\s>]/i;

/**
 * A fragment of mermaid's `.html()` writes. SVG markup (an icon is `<g><svg ...>`) is sanitized inside an `<svg>` wrapper, so
 * the HTML parser puts `<g>` in the SVG namespace (as a body child it is an unknown HTML element and DOMPurify drops it); the
 * wrapper is removed again. Labels and math are HTML and are sanitized as they are.
 */
export function sanitizeFragment(purify: PurifyApi, html: string): string {
  if (!SVG_FRAGMENT.test(html)) return String(purify.sanitize(html, PURIFY_CONFIG));
  const out = String(purify.sanitize(`<svg>${html}</svg>`, PURIFY_CONFIG));
  return out.startsWith("<svg>") && out.endsWith("</svg>") ? out.slice(5, -6) : "";
}

let apiPromise: Promise<Loaded> | null = null;
let counter = 0;
/** Renders run one after another: mermaid keeps global state (config, one scratch element per id) and is not reentrant. */
let queue: Promise<unknown> = Promise.resolve();

function loadMermaid(): Promise<Loaded> {
  apiPromise ??= Promise.all([import("mermaid"), import("dompurify")]).then(([mermaid, purify]) => ({
    api: mermaid.default,
    purify: (html: string) => sanitizeFragment(purify.default, html),
  }));
  apiPromise.catch(() => {
    apiPromise = null; // a failed chunk load may be retried by the next diagram
  });
  return apiPromise;
}

const FONT = "system-ui, -apple-system, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif";

function configure(api: MermaidApi, scheme: Scheme, maxTextSize: number): void {
  api.initialize({
    startOnLoad: false,
    // `strict` encodes HTML in labels and disables click handlers; a diagram directive cannot override it (it is a secure key).
    securityLevel: "strict",
    suppressErrorRendering: true,
    // What a diagram's own `%%{init}%%` directive may not change: the keys mermaid already protects, plus the CSS it would
    // append to the theme, the theme itself (the app's light or dark scheme decides) and the switch back to HTML labels.
    secure: ["secure", "securityLevel", "startOnLoad", "maxTextSize", "suppressErrorRendering", "maxEdges", "themeCSS", "fontFamily", "theme", "htmlLabels"],
    theme: scheme === "dark" ? "dark" : "default",
    maxTextSize,
    fontFamily: FONT,
    // Labels are SVG <text>, never <foreignObject> HTML: an SVG shown as an image renders those inconsistently (Safari).
    htmlLabels: false,
    flowchart: { htmlLabels: false, useMaxWidth: false },
    class: { htmlLabels: false, useMaxWidth: false },
    state: { useMaxWidth: false },
    sequence: { useMaxWidth: false },
    er: { useMaxWidth: false },
    gantt: { useMaxWidth: false },
    journey: { useMaxWidth: false },
    pie: { useMaxWidth: false },
    gitGraph: { useMaxWidth: false },
    mindmap: { useMaxWidth: false },
    requirement: { useMaxWidth: false },
  });
}

/** The SVG comes from HTML serialization: `&nbsp;` (a U+00A0 in a label) is not an XML entity, and the image is parsed as XML. */
const xmlEntities = (svg: string): string => svg.replace(/&nbsp;/g, "&#160;");

/** The first `<svg ...>` tag of the output with an explicit pixel size from its viewBox, or null when there is none. */
export function sizeSvg(svg: string): { svg: string; width: number; height: number } | null {
  const tag = /<svg\b[^>]*>/.exec(svg);
  if (!tag) return null;
  const box = /\bviewBox="\s*(-?[\d.]+)[\s,]+(-?[\d.]+)[\s,]+([\d.]+)[\s,]+([\d.]+)\s*"/.exec(tag[0]);
  const width = box ? Math.ceil(Number(box[3])) : 0;
  const height = box ? Math.ceil(Number(box[4])) : 0;
  if (!(width > 0 && height > 0) || width > 20000 || height > 20000) return null;
  // A standalone image is parsed as XML: `xlink:href` (links, markers) needs its namespace declared on the root.
  const xlink = /\bxmlns:xlink=/.test(tag[0]) ? "" : ' xmlns:xlink="http://www.w3.org/1999/xlink"';
  const rewritten = tag[0]
    .replace(/\s(?:width|height)="[^"]*"/g, "")
    .replace(/\sstyle="[^"]*"/, "")
    .replace(/^<svg\b/, `<svg width="${width}" height="${height}"${xlink}`);
  return { svg: xmlEntities(svg.slice(0, tag.index) + rewritten + svg.slice(tag.index + tag[0].length)), width, height };
}

/** `svg` with the diverted theme CSS (see captureMermaidStyle) as the first child of the root element. */
function withStyle(svg: string, rules: string): string {
  if (rules === "") return svg;
  const css = rules.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return svg.replace(/<svg\b[^>]*>/, (tag) => `${tag}<style>${css}</style>`);
}

const CYTOSCAPE_STYLE_ID = "__________cytoscape_stylesheet";

/**
 * The mindmap and architecture layouts use cytoscape, which appends `<style id="__________cytoscape_stylesheet">` with one
 * rule to <head> unless an element with that id exists (a runtime <style> with text that `style-src 'self'` blocks). An
 * empty element with that id (the empty-text hash of the CSP allows it) keeps cytoscape from adding its own, and the same
 * rule is adopted as a constructed stylesheet, which the CSP does not govern.
 */
function holdCytoscapeStyle(): void {
  if (document.getElementById(CYTOSCAPE_STYLE_ID)) return;
  const marker = document.createElement("style");
  marker.id = CYTOSCAPE_STYLE_ID;
  document.head.prepend(marker);
  if (typeof CSSStyleSheet === "function" && "adoptedStyleSheets" in document) {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(".__________cytoscape_container { position: relative; }");
    document.adoptedStyleSheets = [sheet, ...document.adoptedStyleSheets];
  }
}

/** Removes what mermaid leaves in the document when it fails half way (`#id` or its wrapper `#did`). */
function cleanUp(id: string): void {
  for (const selector of [`#d${id}`, `#${id}`]) document.querySelector(selector)?.remove();
}

const shortMessage = (error: unknown): string => {
  const text = error instanceof Error ? error.message : typeof error === "string" ? error : "unknown error";
  const line = text.split("\n").find((part) => part.trim() !== "") ?? "unknown error";
  return line.trim().slice(0, 120);
};

/** Set by the timeout: the render was given up on and must not start anything more (above all, no capture). */
interface Cancel {
  cancelled: boolean;
}

const CANCELLED: MermaidRender = { ok: false, error: "timed out" };

async function renderNow(id: string, source: string, scheme: Scheme, maxTextSize: number, cancel: Cancel): Promise<MermaidRender> {
  try {
    const { api, purify } = await loadMermaid();
    if (cancel.cancelled) return CANCELLED;
    configure(api, scheme, maxTextSize);
    holdCytoscapeStyle();
    await api.parse(source, { suppressErrors: false });
    if (cancel.cancelled) return CANCELLED;
    const { value, rules } = await captureMermaidStyle(id, () => api.render(id, source), purify);
    const sized = sizeSvg(withStyle(value.svg, rules));
    if (!sized) return { ok: false, error: "the diagram has no size" };
    return { ok: true, ...sized };
  } catch (error) {
    return { ok: false, error: shortMessage(error) };
  } finally {
    cleanUp(id);
  }
}

/**
 * Resolves to a failure after RENDER_TIMEOUT_MS. The stuck render is cut off: its capture is released (a late write by it
 * throws again, the default policy refuses everything) and its scratch element is removed.
 */
function withTimeout(id: string, cancel: Cancel, render: Promise<MermaidRender>): Promise<MermaidRender> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<MermaidRender>((resolve) => {
    timer = setTimeout(() => {
      cancel.cancelled = true;
      releaseMermaidCapture(id);
      cleanUp(id);
      resolve({ ok: false, error: "timed out" });
    }, RENDER_TIMEOUT_MS);
  });
  return Promise.race([render, late]).finally(() => clearTimeout(timer));
}

/** Draws `source`. Never throws: a parse or layout error comes back as `{ ok: false, error }` with a one-line message. */
export function renderMermaid(source: string, scheme: Scheme, maxTextSize: number): Promise<MermaidRender> {
  const run = queue.then(() => {
    const id = `zptah-mermaid-${++counter}`;
    const cancel: Cancel = { cancelled: false };
    return withTimeout(id, cancel, renderNow(id, source, scheme, maxTextSize, cancel));
  });
  queue = run;
  return run;
}
