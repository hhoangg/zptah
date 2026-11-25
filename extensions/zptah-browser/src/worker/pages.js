// page.snapshot, page.read, page.find and page.wait (spec 6.1). Reads run in the content
// script of every readable frame; the worker stitches frames, pages the text by bytes and
// adds the tab-level hints (hidden tab, unrendered page).
import { DEFAULT_DEADLINE_MS, ZptahError } from "./errors.js";
import { eligibleFrames, frameAllowed, runInFrames } from "./inject.js";
import { requireReadable } from "./origins.js";
import { pageText } from "./paging.js";
import { getReadableTab, tabUrl } from "./tabs.js";

const SNAPSHOT_BYTES = 60000;
const READ_BYTES = 40000;
const WAIT_POLL_MS = 250;
const WAIT_MARGIN_MS = 1500;
const CACHE_TABS = 8;
const CACHE_BYTES = 8_000_000;
const SLOT_RE = /^([ ]*)\u0000slot:(\d+)\u0000$/;

/** `e12` or `f3e12` to its frame and local ref. @param {unknown} ref */
export function parseRef(ref) {
  const m = /^(?:f(\d+))?(e\d+)$/.exec(String(ref));
  if (!m) throw new ZptahError("E_STALE_REF", "That ref is not valid. Take a new snapshot.");
  return { frameId: m[1] ? Number(m[1]) : 0, ref: m[2] };
}

/** @param {string} text @param {number} frameId */
export function prefixRefs(text, frameId) {
  return frameId === 0 ? text : text.replace(/\[ref=(e\d+)\]/g, `[ref=f${frameId}$1]`);
}

/**
 * Puts the text of child frames under the iframe nodes of their parents.
 * @param {number} frameId
 * @param {Map<number, any>} results @param {Map<number, import("./inject.js").FrameInfo[]>} children
 * @returns {string}
 */
export function stitch(frameId, results, children) {
  const r = results.get(frameId);
  if (!r || typeof r.text !== "string") return "";
  const kids = [...(children.get(frameId) ?? [])].filter((k) => results.has(k.frameId));
  /** @type {Set<number>} */
  const used = new Set();
  const out = [];
  for (const line of prefixRefs(r.text, frameId).split("\n")) {
    const m = SLOT_RE.exec(line);
    if (!m) {
      out.push(line);
      continue;
    }
    const src = String(r.iframes?.[Number(m[2])]?.src ?? "");
    const child = kids.find((k) => !used.has(k.frameId) && k.url === src) ?? kids.find((k) => !used.has(k.frameId));
    if (!child) continue;
    used.add(child.frameId);
    const inner = stitch(child.frameId, results, children);
    if (inner) for (const l of inner.split("\n")) out.push(`${m[1]}${l}`);
  }
  return out.join("\n").replace(/\u0000/g, "");
}

/**
 * Drops the answers of frames whose address, as the page reports it now, may not be read. A
 * main frame that moved to a blocked page fails the whole call.
 * @param {any} entry @param {{ frameId: number, result: any }[]} answers
 */
export function vet(entry, answers) {
  return answers.filter((a) => {
    const url = a.result?.url;
    if (typeof url !== "string") return true;
    if (a.frameId === 0) {
      requireReadable(entry, url);
      return true;
    }
    return frameAllowed(entry, url);
  });
}

/** Hidden pages that show (almost) nothing: spec 6.4 F7. @param {number} controls */
const looksUnrendered = (controls) => controls <= 2;

/**
 * @param {object} deps
 * @param {any} deps.api
 * @param {() => number} deps.now
 * @param {(ms: number) => Promise<void>} deps.sleep
 */
export function createPages({ api, now, sleep }) {
  /** @type {Map<number, number>} */
  const gens = new Map();
  /** @type {Map<number, { generation: number, url: string, text: string, title: string, hidden_dropped: number, visibility: string, unrendered: boolean }>} */
  const snaps = new Map();

  /** Keeps the paged texts of a few tabs only. @param {number} tabId @param {any} value */
  const remember = (tabId, value) => {
    snaps.delete(tabId);
    snaps.set(tabId, value);
    let total = 0;
    for (const v of snaps.values()) total += v.text.length;
    for (const key of snaps.keys()) {
      if (snaps.size <= CACHE_TABS && total <= CACHE_BYTES) break;
      if (key === tabId) continue;
      total -= snaps.get(key).text.length;
      snaps.delete(key);
    }
  };

  /** @param {number} tabId */
  const nextGeneration = (tabId) => {
    const g = Math.max((gens.get(tabId) ?? 0) + 1, now());
    gens.set(tabId, g);
    return g;
  };

  /** @param {any} entry @param {number} tabId */
  async function frameSet(entry, tabId) {
    const frames = await eligibleFrames(api, tabId, entry);
    /** @type {Map<number, import("./inject.js").FrameInfo[]>} */
    const children = new Map();
    for (const f of frames) {
      if (f.frameId === 0) continue;
      children.set(f.parentFrameId, [...(children.get(f.parentFrameId) ?? []), f]);
    }
    const ids = frames.map((f) => f.frameId);
    if (!ids.includes(0)) ids.unshift(0);
    return { ids, children };
  }

  /** @param {any} entry @param {any} p */
  async function snapshot(entry, p) {
    const tab = await getReadableTab(api, entry, p.tab);
    const max = p.max_bytes ?? SNAPSHOT_BYTES;
    if (p.from !== undefined) {
      const c = snaps.get(tab.id);
      if (!c || p.generation === undefined || c.generation !== p.generation || c.url !== tabUrl(tab)) {
        throw new ZptahError("E_STALE_REF", "The page changed since that snapshot. Take a new snapshot.");
      }
      return { ...pageText(c.text, p.from, max), url: c.url, title: c.title, generation: c.generation, hidden_dropped: c.hidden_dropped, visibility: c.visibility, ...(c.unrendered ? { unrendered: true } : {}) };
    }
    const mode = p.mode === "interactive" ? "interactive" : "full";
    /** @type {string} */
    let text;
    let generation;
    let hiddenDropped = 0;
    let controls = 0;
    let visibility = "visible";
    let url = tabUrl(tab);
    let title = String(tab.title ?? "");
    if (p.scope_ref !== undefined) {
      const { frameId, ref } = parseRef(p.scope_ref);
      const allowed = await frameSet(entry, tab.id);
      if (!allowed.ids.includes(frameId)) throw new ZptahError("E_STALE_REF", "That ref is not in a readable frame. Take a new snapshot.");
      generation = gens.get(tab.id);
      if (generation === undefined) throw new ZptahError("E_STALE_REF", "Take a snapshot first.");
      const [res] = vet(entry, await runInFrames(api, tab.id, [frameId], "snapshot", { generation, mode, depth: p.depth, scope: ref }));
      if (!res || res.result.stale) throw new ZptahError("E_STALE_REF", "That ref is gone. Take a new snapshot.");
      if (res.result.error) throw new ZptahError("E_INVALID", res.result.error);
      text = prefixRefs(res.result.text, frameId).replace(/^[ ]*\u0000slot:\d+\u0000\n?/gm, "").replace(/\u0000/g, "");
      hiddenDropped = res.result.hidden_dropped;
      controls = res.result.controls;
      visibility = res.result.visibility;
      url = res.result.url;
      title = res.result.title;
    } else {
      generation = nextGeneration(tab.id);
      const { ids, children } = await frameSet(entry, tab.id);
      const answers = vet(entry, await runInFrames(api, tab.id, ids, "snapshot", { generation, mode, depth: p.depth }));
      /** @type {Map<number, any>} */
      const results = new Map();
      for (const a of answers) if (!a.result.error && !a.result.stale) results.set(a.frameId, a.result);
      const main = results.get(0);
      if (!main) throw new ZptahError("E_INVALID", answers.find((a) => a.frameId === 0)?.result?.error ?? "the page did not answer");
      text = stitch(0, results, children);
      for (const r of results.values()) {
        hiddenDropped += r.hidden_dropped;
        controls += r.controls;
      }
      visibility = main.visibility;
      url = main.url;
      title = main.title;
    }
    // The page may have moved while we read it: judge the address again.
    const after = await getReadableTab(api, entry, tab.id);
    requireReadable(entry, tabUrl(after));
    const unrendered = visibility === "hidden" && looksUnrendered(controls);
    const first = pageText(text, 0, max);
    if (p.scope_ref !== undefined) {
      // A scoped text is not cached (it would replace the page's full snapshot), so it cannot be continued.
      const { next_from: _unused, ...whole } = first;
      return { ...whole, url, title, generation, hidden_dropped: hiddenDropped, visibility, ...(unrendered ? { unrendered: true } : {}) };
    }
    remember(tab.id, { generation, url: tabUrl(after), text, title, hidden_dropped: hiddenDropped, visibility, unrendered });
    return { ...first, url, title, generation, hidden_dropped: hiddenDropped, visibility, ...(unrendered ? { unrendered: true } : {}) };
  }

  /** @param {any} entry @param {any} p */
  async function read(entry, p) {
    const tab = await getReadableTab(api, entry, p.tab);
    const [main] = vet(entry, await runInFrames(api, tab.id, [0], "read", {}));
    if (!main || main.result.error) throw new ZptahError("E_INVALID", main?.result?.error ?? "the page did not answer");
    const r = main.result;
    await getReadableTab(api, entry, tab.id);
    const hidden = r.visibility === "hidden";
    const unrendered = hidden && looksUnrendered(r.controls);
    const pg = pageText(String(r.markdown ?? ""), p.from ?? 0, p.max_bytes ?? READ_BYTES);
    const { text, ...rest } = pg;
    return { markdown: text, ...rest, url: String(r.url ?? tabUrl(tab)), title: String(r.title ?? tab.title ?? ""), visibility: r.visibility, ...(unrendered ? { unrendered: true } : {}) };
  }

  /** @param {any} entry @param {any} p */
  async function find(entry, p) {
    const tab = await getReadableTab(api, entry, p.tab);
    const { ids } = await frameSet(entry, tab.id);
    const max = p.max ?? 20;
    // Refs are only valid for one generation: any frame that does not hold it (a reload, a new
    // frame) makes a new generation, so a ref never points at a different element.
    let generation = gens.get(tab.id);
    let answers = generation === undefined ? [] : vet(entry, await runInFrames(api, tab.id, ids, "find", { generation, query: p.query, max }));
    if (generation === undefined || answers.some((a) => a.result.stale)) {
      generation = nextGeneration(tab.id);
      answers = vet(entry, await runInFrames(api, tab.id, ids, "find", { generation, query: p.query, max, reset: true }));
    }
    /** @type {any[]} */
    const matches = [];
    let url = tabUrl(tab);
    let title = String(tab.title ?? "");
    for (const a of answers) {
      if (a.result.error || a.result.stale) continue;
      if (a.frameId === 0) {
        url = a.result.url;
        title = a.result.title;
      }
      for (const m of a.result.matches) matches.push({ ...m, ref: a.frameId === 0 ? m.ref : `f${a.frameId}${m.ref}` });
    }
    await getReadableTab(api, entry, tab.id);
    return { url, title, generation, matches: matches.slice(0, max) };
  }

  /** @param {any} entry @param {any} p */
  async function wait(entry, p) {
    const tab0 = await getReadableTab(api, entry, p.tab);
    const start = now();
    const deadline = p.deadline_ms ?? DEFAULT_DEADLINE_MS + (p.time_ms ?? 0);
    const sleepMs = p.time_ms ?? 0;
    if (sleepMs > 0) await sleep(sleepMs);
    const end = Math.max(start + deadline - WAIT_MARGIN_MS, start + sleepMs);
    const wants = p.text !== undefined || p.text_gone !== undefined || p.load === true;
    let tab = tab0;
    for (;;) {
      tab = await getReadableTab(api, entry, p.tab);
      let met = true;
      if (p.load === true && tab.status !== "complete") met = false;
      if (met && (p.text !== undefined || p.text_gone !== undefined)) {
        const { ids } = await frameSet(entry, tab.id);
        const answers = vet(entry, await runInFrames(api, tab.id, ids, "state", { text: p.text, text_gone: p.text_gone }));
        if (p.text !== undefined && !answers.some((a) => a.result.has === true)) met = false;
        if (p.text_gone !== undefined && answers.some((a) => a.result.gone === false)) met = false;
      }
      if (met || !wants) return { met: met || !wants, url: tabUrl(tab), title: String(tab.title ?? "") };
      if (now() + WAIT_POLL_MS > end) return { met: false, url: tabUrl(tab), title: String(tab.title ?? "") };
      await sleep(WAIT_POLL_MS);
    }
  }

  return { snapshot, read, find, wait, gens, snaps };
}
