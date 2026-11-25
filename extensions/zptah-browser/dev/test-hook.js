// Dev build only (scripts/build.mjs copies this file next to main.js and imports it there). The
// store build has neither the file nor the symbol: the build test, the scan and the e2e check it.
// Gives the headless e2e a way to put the worker in the state a Mono's first action leaves it
// in, and to press Stop without a pointer.
// @ts-ignore - resolved next to main.js in the dev build
import { currentRoster } from "./origins.js";

/**
 * @param {{ link: any, read: any, ownership: any, effects: any, stop: any }} ctx
 */
export function installTestHook(ctx) {
  const entryOf = (/** @type {string} */ mono) => currentRoster(ctx.link, ctx.read.slot).find((/** @type {any} */ m) => m.id === mono);
  /** @type {any} */ (globalThis).__zptahTest = {
    /** The Mono owns the tab and its overlay is drawn, as after its first action there. */
    async begin(/** @type {number} */ tab, /** @type {string} */ mono) {
      const entry = entryOf(mono);
      if (!entry) throw new Error("unknown mono");
      ctx.ownership.claim(tab, mono, "adopted");
      await ctx.effects.beforeAct(entry, { tab, frameId: 0, id: "e2e", action: { kind: "scroll" } }, "k");
      return { owner: ctx.ownership.ownerOf(tab), overlay: ctx.effects.has(tab) };
    },
    state(/** @type {number} */ tab) {
      return { owner: ctx.ownership.ownerOf(tab), overlay: ctx.effects.has(tab), takenBack: ctx.ownership.takenBackAt(tab) !== null };
    },
    /** What pressing the overlay's Stop does, without the click. */
    stopTab(/** @type {number} */ tab) {
      return ctx.stop.stopTab(tab);
    },
  };
}
