// On-demand injection of the content script through chrome.scripting. Never activates or
// focuses a tab or window; reads and DOM work need neither.
import { ZptahError } from "./errors.js";
import { checkUrl, isChallengeFrame } from "./origins.js";

export const CONTENT_FILE = "src/content/index.js";

/** @typedef {{ frameId: number, parentFrameId: number, url: string }} FrameInfo */

/** @param {unknown} err @returns {ZptahError} */
export function mapChromeError(err) {
  if (err instanceof ZptahError) return err;
  const m = String(err instanceof Error ? err.message : err);
  if (/no tab with id|tab was closed|cannot find|no frame with id/i.test(m)) return new ZptahError("E_TAB_GONE", "The tab is gone.");
  if (/cannot access|extensions gallery|not allowed|blocked/i.test(m)) return new ZptahError("E_RESTRICTED_URL", "This is a browser page (or a file): zptah Browser never reads it.");
  return new ZptahError("E_INVALID", m.slice(0, 200));
}

/**
 * The frames of a tab whose address may be read, parent frames first.
 * @param {any} api @param {number} tabId @param {import("./origins.js").RosterEntry | null} entry
 * @returns {Promise<FrameInfo[]>}
 */
export async function eligibleFrames(api, tabId, entry) {
  /** @type {FrameInfo[]} */
  let frames;
  try {
    frames = (await api.webNavigation.getAllFrames({ tabId })) ?? [];
  } catch (err) {
    throw mapChromeError(err);
  }
  const ok = frames.filter((f) => {
    if (f.frameId === 0) return true;
    if (f.url === "about:srcdoc" || f.url === "about:blank" || f.url === "") return true;
    return checkUrl(entry, f.url) === "ok" && !isChallengeFrame(f.url);
  });
  return ok.sort((a, b) => a.frameId - b.frameId);
}

/**
 * Runs `zptahContent.run(op, args)` in each frame (injecting first when needed). A frame that
 * cannot be reached is left out of the answer.
 * @param {any} api @param {number} tabId @param {number[]} frameIds @param {string} op @param {unknown} args
 * @returns {Promise<{ frameId: number, result: any }[]>}
 */
export async function runInFrames(api, tabId, frameIds, op, args) {
  const one = async (/** @type {number} */ frameId) => {
    const target = { tabId, frameIds: [frameId] };
    const have = await api.scripting.executeScript({ target, func: () => /** @type {any} */ (globalThis).zptahContent?.version ?? 0 });
    if (!have?.[0] || !have[0].result) await api.scripting.executeScript({ target, files: [CONTENT_FILE] });
    const out = await api.scripting.executeScript({
      target,
      func: (/** @type {string} */ o, /** @type {unknown} */ a) => /** @type {any} */ (globalThis).zptahContent.run(o, a),
      args: [op, args],
    });
    return { frameId, result: out?.[0]?.result };
  };
  const settled = await Promise.allSettled(frameIds.map(one));
  /** @type {{ frameId: number, result: any }[]} */
  const done = [];
  let mainError = null;
  settled.forEach((s, i) => {
    if (s.status === "fulfilled" && s.value.result !== undefined && s.value.result !== null) done.push(s.value);
    else if (frameIds[i] === 0) mainError = s.status === "rejected" ? s.reason : new Error("the page did not answer");
  });
  if (mainError && !done.some((d) => d.frameId === 0)) throw mapChromeError(mainError);
  return done;
}
