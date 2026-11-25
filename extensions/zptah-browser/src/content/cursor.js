// The Mono's virtual cursor (spec 10): one closed shadow root in a popover host, drawn only
// while the Mono acts. The worker decides whether it may be shown (switches, site, ownership);
// this file only draws what it is told and never reads page text.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});

  const VERBS = {
    en: { click: "Clicking", type: "Typing", fill: "Typing", select: "Choosing", press: "Pressing", hover: "Hovering", scroll: "Scrolling", drag: "Dragging", upload: "Uploading" },
    vi: { click: "Đang bấm", type: "Đang nhập", fill: "Đang nhập", select: "Đang chọn", press: "Đang nhấn", hover: "Đang di chuột", scroll: "Đang cuộn", drag: "Đang kéo", upload: "Đang tải lên" },
  };
  const LABEL_MAX = 48;
  const FADE_MS = 1200;
  const MOVE_MS = 250;

  /** @param {string} hex @returns {number} relative luminance (WCAG 2) */
  function luminance(hex) {
    const n = Number.parseInt(hex.slice(1), 16);
    const lin = (/** @type {number} */ c) => {
      const s = c / 255;
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * lin((n >> 16) & 255) + 0.7152 * lin((n >> 8) & 255) + 0.0722 * lin(n & 255);
  }

  /** @param {string} a @param {string} b @returns {number} contrast ratio, 1 to 21 */
  function ratio(a, b) {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
  }

  /** Black or white text, whichever reads better on `color`. @param {string} color */
  function textOn(color) {
    return ratio(color, "#000000") >= ratio(color, "#ffffff") ? "#000000" : "#ffffff";
  }

  /** @param {string} text @param {number} max */
  function clip(text, max) {
    const chars = Array.from(text);
    return chars.length <= max ? text : `${chars.slice(0, max - 1).join("")}…`;
  }

  /**
   * `🐸 Mira · Clicking "Add to cart"`, at most 48 characters. The target is the element's
   * accessible name; typed text is never passed here.
   * @param {{ emoji: string, name: string, kind: string, target?: string, lang?: string }} a
   */
  function labelText(a) {
    const verbs = a.lang === "vi" ? VERBS.vi : VERBS.en;
    const verb = /** @type {Record<string, string>} */ (verbs)[a.kind] ?? verbs.click;
    const head = `${a.emoji} ${a.name} · ${verb}`;
    const target = typeof a.target === "string" ? a.target.replace(/\s+/g, " ").trim() : "";
    const full = target ? `${head} "${target}"` : head;
    if (Array.from(full).length <= LABEL_MAX) return full;
    const room = LABEL_MAX - Array.from(head).length - 4; // space, two quotes, ellipsis
    if (target && room >= 3) return `${head} "${clip(target, room)}"`.slice(0, 200);
    return clip(head, LABEL_MAX);
  }

  /** @type {any} */
  let host = null;
  /** @type {any} */
  let parts = null;
  /** @type {any} */
  let fadeTimer = null;
  let suspended = false;

  function reduced() {
    try {
      return root.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
    } catch {
      return false;
    }
  }

  function build() {
    const doc = root.document;
    const el = doc.createElement("div");
    el.setAttribute("popover", "manual");
    el.setAttribute("aria-hidden", "true");
    el.setAttribute("inert", "");
    el.style.cssText =
      "all: initial !important; position: fixed !important; inset: 0 auto auto 0 !important; width: 0 !important; height: 0 !important; pointer-events: none !important; z-index: 2147483647 !important; contain: layout style !important; overflow: visible !important; border: 0 !important; padding: 0 !important; margin: 0 !important; background: transparent !important;";
    const shadow = el.attachShadow({ mode: "closed" });
    const style = doc.createElement("style");
    style.textContent =
      ".c{position:fixed;left:0;top:0;pointer-events:none;will-change:transform}" +
      ".c *{pointer-events:none}" +
      ".a{display:block;width:22px;height:22px}" +
      ".l{position:absolute;left:18px;top:20px;padding:3px 9px;border-radius:999px;font:600 12px/1.3 system-ui,sans-serif;white-space:nowrap;box-shadow:0 1px 4px rgba(0,0,0,.35)}" +
      ".r{position:fixed;left:0;top:0;width:34px;height:34px;margin:-17px 0 0 -17px;border-radius:50%;opacity:0;pointer-events:none}" +
      "@keyframes rip{from{opacity:.6;transform:scale(.3)}to{opacity:0;transform:scale(1.4)}}";
    const cursor = doc.createElement("div");
    cursor.className = "c";
    const arrow = doc.createElement("div");
    arrow.className = "a";
    const label = doc.createElement("div");
    label.className = "l";
    const ripple = doc.createElement("div");
    ripple.className = "r";
    cursor.append(arrow, label);
    shadow.append(style, cursor, ripple);
    parts = { cursor, arrow, label, ripple, fresh: true };
    return el;
  }

  /** Our own arrow, filled with the Mono's color and outlined in white. @param {string} color */
  function arrowSvg(color) {
    return `<svg xmlns="http://www.w3.org/2000/svg" width="22" height="22" viewBox="0 0 22 22"><path d="M3 2 L3 17 L7.2 13.2 L10 19.5 L12.6 18.4 L9.9 12.2 L15.5 12.2 Z" fill="${color}" stroke="#ffffff" stroke-width="1.5" stroke-linejoin="round"/></svg>`;
  }

  /** @param {boolean} show */
  function present(show) {
    if (!host) return;
    const doc = root.document;
    if (!host.isConnected) doc.documentElement.appendChild(host);
    if (!show) return;
    try {
      if (host.matches(":popover-open")) host.hidePopover();
    } catch {
      // not open
    }
    try {
      host.showPopover();
    } catch {
      // The Popover API is missing: the fixed host still draws, just not in the top layer.
    }
  }

  /**
   * Moves the cursor to (x, y) in this frame's viewport and shows the label.
   * @param {{ x: number, y: number, color: string, emoji: string, name: string, kind: string, target?: string, lang?: string, click?: boolean }} a
   */
  function move(a) {
    if (!/^#[0-9a-f]{6}$/i.test(a.color) || !Number.isFinite(a.x) || !Number.isFinite(a.y)) return { shown: false };
    if (!host) host = build();
    const p = parts;
    const first = p.fresh;
    p.fresh = false;
    p.arrow.innerHTML = arrowSvg(a.color);
    p.label.textContent = labelText(a);
    p.label.style.background = a.color;
    p.label.style.color = textOn(a.color);
    p.ripple.style.background = a.color;
    const fast = first || reduced();
    // A new cursor starts at the target with the transition off, so it does not fly in from 0,0.
    p.cursor.style.transition = fast ? "none" : `transform ${MOVE_MS}ms ease-out`;
    p.cursor.style.transform = `translate(${Math.round(a.x)}px, ${Math.round(a.y)}px)`;
    present(!suspended);
    if (host && suspended) host.style.setProperty("visibility", "hidden", "important");
    if (a.click === true && !reduced()) {
      p.ripple.style.transform = `translate(${Math.round(a.x)}px, ${Math.round(a.y)}px)`;
      p.ripple.style.animation = "none";
      p.ripple.style.animation = "rip 450ms ease-out";
    }
    if (fadeTimer !== null) root.clearTimeout(fadeTimer);
    fadeTimer = root.setTimeout(hide, FADE_MS + (fast ? 0 : MOVE_MS));
    return { shown: true, label: p.label.textContent };
  }

  /** Removes the host, so an idle page carries no trace. */
  function hide() {
    if (fadeTimer !== null) root.clearTimeout(fadeTimer);
    fadeTimer = null;
    if (host) {
      try {
        host.hidePopover?.();
      } catch {
        // not open
      }
      host.remove();
    }
    host = null;
    parts = null;
    return { shown: false };
  }

  /** Hidden for a screenshot and restored after (spec 10). @param {boolean} on */
  function suspend(on) {
    suspended = on;
    if (host) {
      if (on) host.style.setProperty("visibility", "hidden", "important");
      else host.style.removeProperty("visibility");
    }
    return {};
  }

  ns.cursor = { move, hide, suspend, textOn, ratio, labelText, isShown: () => host !== null };
})();
