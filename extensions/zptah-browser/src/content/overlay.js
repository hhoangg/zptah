// The in-tab glow and the Stop button (spec 10a). One host per tab (top frame only), a closed
// shadow root, shown as a popover in the top layer. Everything but the button ignores the
// pointer. The button acts only on trusted clicks, so a page script cannot stop the Mono. The
// overlay never shows page text and offers no "Allow": only Stop.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});

  const TEXT = {
    en: {
      stop: (/** @type {string} */ name) => `Stop ${name}`,
      modal: "Stop from the zptah icon in the toolbar",
      full: "Press Esc, then stop from the zptah icon",
    },
    vi: {
      stop: (/** @type {string} */ name) => `Dừng ${name}`,
      modal: "Dừng bằng biểu tượng zptah trên thanh công cụ",
      full: "Nhấn Esc, rồi dừng bằng biểu tượng zptah",
    },
  };
  const TICK_MS = 1000;
  const BASE_STYLE = {
    all: "initial", position: "fixed", inset: "0", width: "100vw", height: "100vh", "pointer-events": "none",
    "z-index": "2147483647", display: "block", opacity: "1", contain: "layout style", overflow: "visible",
    border: "0", padding: "0", margin: "0", background: "transparent",
  };

  /** @type {any} */
  let host = null;
  /** @type {any} */
  let parts = null;
  /** @type {any} */
  let config = null;
  /** @type {any} */
  let timer = null;
  /** @type {any} */
  let watcher = null;
  /** @type {any} */
  let sender = null;
  let mode = "button";
  let pulsing = false;
  let suspended = false;

  /** @returns {Record<string, string>} the inline style the host is held to; hidden only while a screenshot is taken */
  const hostStyle = () => ({ ...BASE_STYLE, visibility: suspended ? "hidden" : "visible" });

  /** Sets every property with !important. Returns true when something had to be put back. @param {any} el @param {boolean} [first] */
  function applyStyle(el, first = false) {
    let changed = false;
    const style = hostStyle();
    // Only these are compared: shorthands such as inset read back in another form.
    const watched = ["position", "display", "visibility", "opacity", "pointer-events", "z-index"];
    for (const [name, value] of Object.entries(style)) {
      if (!first && !watched.includes(name)) continue;
      if (first || el.style.getPropertyValue(name) !== value || el.style.getPropertyPriority(name) !== "important") {
        el.style.setProperty(name, value, "important");
        changed = true;
      }
    }
    return changed;
  }

  /** @param {unknown} message */
  function send(message) {
    if (sender) return sender(message);
    return root.chrome?.runtime?.sendMessage?.(message);
  }

  /** @param {string} hex @param {number} alpha */
  function rgba(hex, alpha) {
    const n = Number.parseInt(hex.slice(1), 16);
    return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${alpha})`;
  }

  function reduced() {
    try {
      return root.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches === true;
    } catch {
      return false;
    }
  }

  /** Which state the page is in: a modal dialog and fullscreen make the button inert (M0.8). */
  function pageMode() {
    const doc = root.document;
    if (doc.fullscreenElement) return "full";
    try {
      if (doc.querySelector(":modal")) return "modal";
    } catch {
      // :modal is not supported
    }
    return "button";
  }

  function lang() {
    return config?.lang === "vi" ? TEXT.vi : TEXT.en;
  }

  function build() {
    const doc = root.document;
    const el = doc.createElement("div");
    el.setAttribute("popover", "manual");
    el.setAttribute("aria-hidden", "true");
    el.setAttribute("inert", "");
    applyStyle(el, true);
    const shadow = el.attachShadow({ mode: "closed" });
    const style = doc.createElement("style");
    style.textContent =
      ".g{position:fixed;inset:0;pointer-events:none;border-radius:0}" +
      "@keyframes pulse{0%,100%{opacity:.55}50%{opacity:1}}" +
      ".w{position:fixed;left:50%;bottom:24px;transform:translateX(-50%);pointer-events:none;display:flex;justify-content:center}" +
      ".b{pointer-events:auto;display:inline-flex;align-items:center;gap:8px;padding:9px 16px;border:0;border-radius:999px;font:600 14px/1 system-ui,sans-serif;cursor:pointer;box-shadow:0 2px 10px rgba(0,0,0,.4);white-space:nowrap}" +
      ".n{pointer-events:none;padding:8px 14px;border-radius:999px;font:500 13px/1.2 system-ui,sans-serif;white-space:nowrap;box-shadow:0 2px 10px rgba(0,0,0,.4)}" +
      ".i{width:12px;height:12px;display:inline-block;border-radius:2px}";
    const glow = doc.createElement("div");
    glow.className = "g";
    const wrap = doc.createElement("div");
    wrap.className = "w";
    const button = doc.createElement("button");
    button.className = "b";
    button.setAttribute("type", "button");
    const note = doc.createElement("div");
    note.className = "n";
    wrap.append(button, note);
    shadow.append(style, glow, wrap);

    // Only a real click of the user counts. Events a page script dispatches (it cannot reach
    // the closed root, but it can see the retargeted host) are not trusted.
    button.addEventListener("pointerdown", (/** @type {any} */ e) => {
      if (e.isTrusted !== true) return;
      e.stopPropagation();
    });
    button.addEventListener("click", (/** @type {any} */ e) => {
      if (e.isTrusted !== true) return;
      e.stopPropagation();
      void send({ type: "zptah.overlay_stop" });
    });
    parts = { glow, wrap, button, note };
    return el;
  }

  /** Paints the current config and mode into the shadow root. */
  function paint() {
    if (!parts || !config) return;
    const t = lang();
    const color = config.color;
    const fg = ns.cursor.textOn(color);
    parts.glow.style.boxShadow = `inset 0 0 12px 4px ${rgba(color, 0.35)}`;
    parts.glow.style.animation = pulsing && !reduced() ? "pulse 2s ease-in-out infinite" : "none";
    parts.button.style.background = color;
    parts.button.style.color = fg;
    parts.note.style.background = color;
    parts.note.style.color = fg;
    parts.button.textContent = "";
    const icon = root.document.createElement("span");
    icon.className = "i";
    icon.style.background = fg;
    parts.button.append(icon, `${config.emoji} ${t.stop(config.name)}`);
    parts.button.style.display = mode === "button" ? "inline-flex" : "none";
    parts.note.style.display = mode === "button" ? "none" : "block";
    parts.note.textContent = mode === "full" ? t.full : t.modal;
  }

  /** Puts the host back in the document if the page removed it, and on top of the top layer. @param {boolean} [reshow] */
  function ensure(reshow = false) {
    if (!host || !config) return;
    const doc = root.document;
    let dirty = reshow;
    if (!host.isConnected) {
      doc.documentElement.appendChild(host);
      dirty = true;
    }
    if (applyStyle(host)) dirty = true;
    const now = pageMode();
    if (now !== mode) {
      mode = now;
      paint();
      dirty = true;
    }
    let open = false;
    try {
      open = host.matches(":popover-open");
    } catch {
      open = true;
    }
    if (!open) dirty = true;
    if (dirty) {
      try {
        if (open) host.hidePopover();
      } catch {
        // already closed
      }
      try {
        host.showPopover();
      } catch {
        // No Popover API: the fixed host still draws.
      }
    }
  }

  /**
   * Shows (or updates) the overlay.
   * @param {{ color: string, emoji: string, name: string, lang?: string, pulse?: boolean }} a
   */
  function show(a) {
    if (!/^#[0-9a-f]{6}$/i.test(a.color)) return { shown: false };
    config = { color: a.color, emoji: String(a.emoji).slice(0, 32), name: String(a.name).slice(0, 64), lang: a.lang };
    pulsing = a.pulse === true;
    if (!host) {
      host = build();
      const doc = root.document;
      watcher = new root.MutationObserver(() => ensure());
      watcher.observe(doc.documentElement, { childList: true });
      watcher.observe(host, { attributes: true });
      doc.addEventListener?.("fullscreenchange", () => ensure(true), true);
      doc.addEventListener?.("toggle", () => ensure(true), true);
      timer = root.setInterval(() => ensure(), TICK_MS);
    }
    mode = pageMode();
    paint();
    ensure(true);
    return { shown: true };
  }

  function hide() {
    if (timer !== null) root.clearInterval(timer);
    timer = null;
    watcher?.disconnect?.();
    watcher = null;
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
    config = null;
    return { shown: false };
  }

  /** Hidden for a screenshot and restored after. @param {boolean} on */
  function suspend(on) {
    suspended = on;
    if (host) applyStyle(host);
    return {};
  }

  ns.overlay = {
    show,
    suspend,
    hide,
    ensure,
    isShown: () => host !== null,
    mode: () => mode,
    setSender: (/** @type {any} */ fn) => {
      sender = fn;
    },
  };
})();
