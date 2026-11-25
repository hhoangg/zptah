// page.screenshot (spec 6.4 "Screenshots", F5). captureVisibleTab only for the active tab of a
// visible window, under a timeout (it hangs on hidden pages); chrome.debugger only when the
// optional permission is granted AND the debugger is already attached to the tab. Never
// activates a tab or window. The PNG is scaled down until it fits `max_bytes`.
import { ZptahError } from "./errors.js";
import { runInFrames } from "./inject.js";
import { requireReadable } from "./origins.js";
import { parseRef } from "./pages.js";
import { getReadableTab, tabUrl } from "./tabs.js";

export const CAPTURE_TIMEOUT_MS = 2000;
const CAPTURE_GAP_MS = 500;
/** A base64 PNG must fit B64Bytes (1 MiB) with the rest of the frame. */
export const MAX_PNG_BYTES = 750000;
const DEFAULT_MAX = 700000;
const MAX_FULL_PAGE_HEIGHT = 16384;
const BACKGROUND_TEXT = "The tab is in the background; use browser_snapshot, or ask the user to show the tab";

/** @param {string} b64 @returns {Uint8Array} */
export function fromBase64(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
  return out;
}

/** @param {Uint8Array} bytes @returns {string} */
export function toBase64(bytes) {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** Width and height from the IHDR chunk, or null when this is not a PNG. @param {Uint8Array} b */
export function pngSize(b) {
  const sig = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
  if (b.length < 24 || sig.some((v, i) => b[i] !== v)) return null;
  const dv = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const width = dv.getUint32(16);
  const height = dv.getUint32(20);
  return width > 0 && height > 0 ? { width, height } : null;
}

/**
 * Browser image work. `render` decodes a PNG, crops (device pixels) and scales (percent).
 * @typedef {{ render(png: Uint8Array, opts: { crop?: { x: number, y: number, width: number, height: number }, scalePct: number }): Promise<{ bytes: Uint8Array, width: number, height: number }> }} Imaging
 */

/** @returns {Imaging} */
export function canvasImaging() {
  return {
    async render(png, { crop, scalePct }) {
      const bmp = await createImageBitmap(new Blob([/** @type {BlobPart} */ (png)], { type: "image/png" }));
      const sx = Math.max(0, Math.min(bmp.width - 1, Math.round(crop?.x ?? 0)));
      const sy = Math.max(0, Math.min(bmp.height - 1, Math.round(crop?.y ?? 0)));
      const sw = Math.max(1, Math.min(bmp.width - sx, Math.round(crop?.width ?? bmp.width)));
      const sh = Math.max(1, Math.min(bmp.height - sy, Math.round(crop?.height ?? bmp.height)));
      const w = Math.max(1, Math.round((sw * scalePct) / 100));
      const h = Math.max(1, Math.round((sh * scalePct) / 100));
      const canvas = new OffscreenCanvas(w, h);
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new ZptahError("E_INVALID", "no canvas");
      ctx.drawImage(bmp, sx, sy, sw, sh, 0, 0, w, h);
      bmp.close();
      const blob = await canvas.convertToBlob({ type: "image/png" });
      return { bytes: new Uint8Array(await blob.arrayBuffer()), width: w, height: h };
    },
  };
}

/**
 * Scales (and crops) until the PNG fits `max` bytes.
 * @param {Imaging} imaging @param {Uint8Array} png @param {number} max
 * @param {{ x: number, y: number, width: number, height: number }} [crop]
 * @returns {Promise<{ bytes: Uint8Array, width: number, height: number, scale_pct?: number }>}
 */
export async function fitPng(imaging, png, max, crop) {
  const size = pngSize(png);
  if (!size) throw new ZptahError("E_INVALID", "the browser returned something that is not a PNG");
  if (!crop && png.length <= max) return { bytes: png, ...size };
  let scale = 100;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const out = await imaging.render(png, { crop, scalePct: scale });
    if (out.bytes.length <= max) return { ...out, ...(scale < 100 ? { scale_pct: scale } : {}) };
    scale = Math.min(scale - 1, Math.floor(scale * Math.sqrt(max / out.bytes.length) * 0.9));
    if (scale < 5) break;
  }
  throw new ZptahError("E_TOO_LARGE", "The picture is too large to send even when scaled down.");
}

/**
 * @param {object} deps
 * @param {any} deps.api
 * @param {Imaging} deps.imaging
 * @param {(ms: number) => Promise<void>} deps.sleep
 * @param {() => number} deps.now
 * @param {(tabId: number) => number | undefined} deps.getGeneration
 * @param {number} [deps.captureTimeoutMs]
 */
export function createScreenshotter({ api, imaging, sleep, now, getGeneration, captureTimeoutMs = CAPTURE_TIMEOUT_MS }) {
  let lastCapture = 0;
  let queue = Promise.resolve();

  /** @param {any} tab */
  async function activeTabId(tab) {
    /** @type {any[]} */
    const tabs = await api.tabs.query({ windowId: tab.windowId });
    return tabs.find((t) => t.active)?.id;
  }

  /** @param {any} tab */
  async function isShown(tab) {
    if (!tab.active) return false;
    try {
      const win = await api.windows.get(tab.windowId);
      if (win?.state === "minimized") return false;
      const [main] = await runInFrames(api, tab.id, [0], "state", {});
      return main?.result?.visible === true;
    } catch {
      return false;
    }
  }

  /** @param {any} tab */
  function captureVisible(tab) {
    const run = async () => {
      const wait = lastCapture + CAPTURE_GAP_MS - now();
      if (wait > 0) await sleep(wait);
      lastCapture = now();
      if ((await activeTabId(tab)) !== tab.id) throw new ZptahError("E_UNSUPPORTED", BACKGROUND_TEXT);
      /** @type {Promise<string>} */
      const shot = api.tabs.captureVisibleTab(tab.windowId, { format: "png" });
      let timer;
      const timeout = new Promise((_, reject) => {
        timer = setTimeout(() => reject(new ZptahError("E_UNSUPPORTED", BACKGROUND_TEXT)), captureTimeoutMs);
      });
      shot.catch(() => {});
      let dataUrl;
      try {
        dataUrl = /** @type {string} */ (await Promise.race([shot, timeout]));
      } finally {
        clearTimeout(timer);
      }
      // A call that settled late can show another tab: check the active tab again.
      if ((await activeTabId(tab)) !== tab.id) throw new ZptahError("E_UNSUPPORTED", "The visible tab changed while the picture was taken. Try again.");
      const comma = dataUrl.indexOf(",");
      return fromBase64(dataUrl.slice(comma + 1));
    };
    const result = queue.then(run, run);
    queue = result.then(() => {}, () => {});
    return result;
  }

  /** @param {any} tab @param {Record<string, unknown> | null} [clip] @param {boolean} [beyond] */
  async function captureDebugger(tab, clip, beyond) {
    const target = { tabId: tab.id };
    const params = { format: "png", ...(clip ? { clip } : {}), ...(beyond ? { captureBeyondViewport: true } : {}) };
    try {
      const out = await api.debugger.sendCommand(target, "Page.captureScreenshot", params);
      return fromBase64(String(out?.data ?? ""));
    } catch (err) {
      throw new ZptahError("E_UNSUPPORTED", `The debugger could not take the picture: ${String(err instanceof Error ? err.message : err).slice(0, 150)}`);
    }
  }

  /** The debugger path needs the optional permission and an attach that already exists. @param {any} tab */
  async function debuggerReady(tab) {
    try {
      if (!(await api.permissions.contains({ permissions: ["debugger"] }))) return false;
      /** @type {any[]} */
      const targets = await api.debugger.getTargets();
      return targets.some((t) => t.tabId === tab.id && t.attached);
    } catch {
      return false;
    }
  }

  /**
   * @param {any} entry
   * @param {{ tab: number, ref?: string, full_page?: boolean, max_bytes?: number }} p
   */
  async function screenshot(entry, p) {
    const tab = await getReadableTab(api, entry, p.tab);
    const max = Math.min(p.max_bytes ?? DEFAULT_MAX, MAX_PNG_BYTES);
    /** @type {{ x: number, y: number, width: number, height: number } | undefined} */
    let rect;
    let dpr = 1;
    if (p.ref !== undefined) {
      const { frameId, ref } = parseRef(p.ref);
      if (frameId !== 0) throw new ZptahError("E_UNSUPPORTED", "A picture of an element inside a frame is not supported. Take one of the whole tab.");
      const generation = getGeneration(tab.id);
      if (generation === undefined) throw new ZptahError("E_STALE_REF", "Take a snapshot first.");
      const [res] = await runInFrames(api, tab.id, [0], "rect", { generation, ref });
      if (!res || res.result.stale) throw new ZptahError("E_STALE_REF", "That ref is gone. Take a new snapshot.");
      const r = res.result;
      const x = Math.max(0, r.x);
      const y = Math.max(0, r.y);
      const width = Math.min(r.width, r.viewport.width - x);
      const height = Math.min(r.height, r.viewport.height - y);
      if (width < 1 || height < 1) throw new ZptahError("E_UNSUPPORTED", "That element is not inside the visible part of the page.");
      rect = { x, y, width, height };
      dpr = r.dpr;
    }
    /** @type {Uint8Array} */
    let png;
    /** @type {"visible" | "debugger"} */
    let method;
    /** @type {{ x: number, y: number, width: number, height: number } | undefined} */
    let crop;
    if (p.full_page === true) {
      if (!(await debuggerReady(tab))) throw new ZptahError("E_UNSUPPORTED", "A full-page picture needs the debugger, which is not attached to this tab. Take a picture of the visible part, or use browser_snapshot.");
      const metrics = await api.debugger.sendCommand({ tabId: tab.id }, "Page.getLayoutMetrics", {});
      const size = metrics?.cssContentSize ?? metrics?.contentSize ?? { width: 1280, height: 800 };
      png = await captureDebugger(tab, { x: 0, y: 0, width: Math.ceil(size.width), height: Math.min(Math.ceil(size.height), MAX_FULL_PAGE_HEIGHT), scale: 1 }, true);
      method = "debugger";
    } else if (await isShown(tab)) {
      png = await captureVisible(tab);
      method = "visible";
      if (rect) crop = { x: rect.x * dpr, y: rect.y * dpr, width: rect.width * dpr, height: rect.height * dpr };
    } else if (await debuggerReady(tab)) {
      png = await captureDebugger(tab, rect ? { ...rect, scale: 1 } : null, false);
      method = "debugger";
    } else {
      throw new ZptahError("E_UNSUPPORTED", BACKGROUND_TEXT);
    }
    const fit = await fitPng(imaging, png, max, crop);
    // The page may have moved while the picture was taken.
    const after = await getReadableTab(api, entry, tab.id);
    requireReadable(entry, tabUrl(after));
    return {
      png_b64: toBase64(fit.bytes),
      width: fit.width,
      height: fit.height,
      method,
      ...(fit.scale_pct !== undefined ? { scale_pct: fit.scale_pct } : {}),
      url: tabUrl(after).slice(0, 8192),
      title: String(after.title ?? "").slice(0, 8192),
    };
  }

  return { screenshot };
}
