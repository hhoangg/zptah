// Registers the read-path methods on the link's dispatcher (tabs.list, page.snapshot,
// page.read, page.find, page.screenshot, page.wait) plus roster.set, which the origin rules
// need. Every request is Mono-scoped, checked against the roster and answered by its deadline.
import { DEFAULT_DEADLINE_MS, errorReply, okReply, withDeadline, ZptahError } from "./errors.js";
import { currentRoster } from "./origins.js";
import { createPages } from "./pages.js";
import { canvasImaging, createScreenshotter } from "./screenshot.js";
import { listTabs } from "./tabs.js";

const HEX32 = /^[0-9a-f]{32}$/;

/**
 * @param {import("./link.js").BrowserLink} link
 * @param {{ api?: any, now?: () => number, sleep?: (ms: number) => Promise<void>, imaging?: import("./screenshot.js").Imaging,
 *   ownerOf?: (tab: number) => string | null, captureTimeoutMs?: number,
 *   dbg?: { send: (tab: number, method: string, params?: Record<string, unknown>) => Promise<any>, isAttached: (tab: number) => boolean } }} [deps]
 */
export function registerReadPath(link, deps = {}) {
  const api = deps.api ?? /** @type {any} */ (globalThis).chrome;
  const now = deps.now ?? Date.now;
  const sleep = deps.sleep ?? ((/** @type {number} */ ms) => new Promise((r) => setTimeout(r, ms)));
  const ownerOf = deps.ownerOf ?? (() => null);
  const slot = { override: /** @type {unknown} */ (null), seen: /** @type {unknown} */ (undefined) };
  const pages = createPages({ api, now, sleep });
  const shots = createScreenshotter({
    api, sleep, now, imaging: deps.imaging ?? canvasImaging(), getGeneration: (t) => pages.gens.get(t), captureTimeoutMs: deps.captureTimeoutMs,
    cdp: deps.dbg?.send, isAttached: deps.dbg?.isAttached,
  });

  /** @param {Record<string, any>} msg */
  function entryFor(msg) {
    if (link.stopped) throw new ZptahError("E_STOPPED", "zptah Browser is stopped.");
    if (typeof msg.mono !== "string" || !HEX32.test(msg.mono)) throw new ZptahError("E_INVALID", "mono is required");
    const entry = currentRoster(link, slot).find((m) => m.id === msg.mono);
    if (!entry) throw new ZptahError("E_UNKNOWN_MONO", "This Mono is not known to the browser.");
    return entry;
  }

  /**
   * @param {string} method
   * @param {(entry: import("./origins.js").RosterEntry, params: any) => Promise<Record<string, unknown>>} fn
   */
  function route(method, fn) {
    link.handlers.set(method, async (msg) => {
      const id = String(msg.id);
      try {
        const entry = entryFor(msg);
        const params = /** @type {any} */ (msg.params ?? {});
        const deadline = Number.isInteger(params.deadline_ms) ? params.deadline_ms : DEFAULT_DEADLINE_MS;
        return okReply(id, await withDeadline(deadline, () => fn(entry, params)));
      } catch (err) {
        return errorReply(id, err);
      }
    });
  }

  link.handlers.set("roster.set", async (msg) => {
    const monos = /** @type {any} */ (msg.params)?.monos;
    if (!Array.isArray(monos)) return errorReply(String(msg.id), new ZptahError("E_INVALID", "monos must be a list"));
    currentRoster(link, slot);
    slot.override = monos;
    return okReply(String(msg.id), {});
  });
  route("tabs.list", (entry, p) => listTabs(api, entry, entry.id, p.all === true, ownerOf));
  route("page.snapshot", (entry, p) => pages.snapshot(entry, p));
  route("page.read", (entry, p) => pages.read(entry, p));
  route("page.find", (entry, p) => pages.find(entry, p));
  route("page.wait", (entry, p) => pages.wait(entry, p));
  route("page.screenshot", (entry, p) => shots.screenshot(entry, p));
  // The act path shares the roster slot, the generation table and the screenshotter.
  return Object.assign(pages, { slot, shots, ownerOf });
}
