// @ts-nocheck - a tiny DOM for the cursor and overlay tests: closed shadow roots, popovers,
// inline styles with priority, events with isTrusted, mutation observers and manual timers.
export function createPage({ modal = false, fullscreen = false } = {}) {
  const state = { modal, fullscreen, openPopovers: new Set(), shown: [], timers: [], observers: [] };

  class Style {
    #m = new Map();
    setProperty(n, v, p = "") {
      this.#m.set(n, { v: String(v), p });
    }
    getPropertyValue(n) {
      return this.#m.get(n)?.v ?? "";
    }
    getPropertyPriority(n) {
      return this.#m.get(n)?.p ?? "";
    }
    removeProperty(n) {
      this.#m.delete(n);
    }
    set background(v) {
      this.setProperty("background", v);
    }
    set color(v) {
      this.setProperty("color", v);
    }
    set display(v) {
      this.setProperty("display", v);
    }
    get display() {
      return this.getPropertyValue("display");
    }
    set boxShadow(v) {
      this.setProperty("box-shadow", v);
    }
    set animation(v) {
      this.setProperty("animation", v);
    }
    set transition(v) {
      this.setProperty("transition", v);
    }
    set transform(v) {
      this.setProperty("transform", v);
    }
    set cssText(v) {
      this.#m.clear();
    }
  }

  class Node_ {
    constructor(tag) {
      this.tag = tag;
      this.children = [];
      this.parent = null;
      this.style = new Style();
      this.attrs = new Map();
      this.listeners = new Map();
      this.className = "";
      this._text = "";
    }
    get isConnected() {
      let n = this;
      while (n.parent) n = n.parent;
      return n === doc.documentElement || n === doc;
    }
    setAttribute(n, v) {
      this.attrs.set(n, String(v));
    }
    getAttribute(n) {
      return this.attrs.get(n) ?? null;
    }
    append(...items) {
      for (const it of items) this.appendChild(typeof it === "string" ? Object.assign(new Node_("#text"), { _text: it }) : it);
    }
    appendChild(c) {
      if (c.parent) c.parent.children = c.parent.children.filter((x) => x !== c);
      c.parent = this;
      this.children.push(c);
      notify(this, "childList");
      return c;
    }
    replaceChildren() {
      this.children = [];
    }
    remove() {
      const p = this.parent;
      if (!p) return;
      p.children = p.children.filter((x) => x !== this);
      this.parent = null;
      state.openPopovers.delete(this);
      notify(p, "childList");
    }
    set textContent(v) {
      this._text = String(v);
      this.children = [];
    }
    get textContent() {
      return this._text + this.children.map((c) => c.textContent).join("");
    }
    set innerHTML(v) {
      this._html = v;
    }
    get innerHTML() {
      return this._html ?? "";
    }
    addEventListener(type, fn) {
      if (!this.listeners.has(type)) this.listeners.set(type, []);
      this.listeners.get(type).push(fn);
    }
    dispatch(type, init = {}) {
      const e = { type, isTrusted: false, stopped: false, stopPropagation() { this.stopped = true; }, preventDefault() {}, ...init };
      for (const fn of this.listeners.get(type) ?? []) fn(e);
      return e;
    }
    attachShadow({ mode }) {
      this._shadow = new Node_("#shadow");
      this._shadow.parent = null;
      this._shadowMode = mode;
      return this._shadow;
    }
    get shadowRoot() {
      return this._shadowMode === "open" ? this._shadow : null;
    }
    matches(sel) {
      if (sel === ":popover-open") return state.openPopovers.has(this);
      return false;
    }
    showPopover() {
      state.openPopovers.add(this);
      state.shown.push(this);
    }
    hidePopover() {
      state.openPopovers.delete(this);
    }
    find(pred, out = []) {
      if (pred(this)) out.push(this);
      for (const c of this.children) c.find(pred, out);
      if (this._shadow) this._shadow.find(pred, out);
      return out;
    }
  }

  function notify(target, type) {
    for (const o of state.observers) if (o.target === target && o.opts[type]) o.fn([{ type, target }]);
  }

  const doc = new Node_("#document");
  doc.documentElement = new Node_("html");
  doc.documentElement.parent = doc;
  doc.body = doc.documentElement.appendChild(new Node_("body"));
  doc.createElement = (tag) => new Node_(tag);
  Object.defineProperty(doc, "fullscreenElement", { get: () => (state.fullscreen ? doc.documentElement : null) });
  doc.querySelector = (sel) => (sel === ":modal" && state.modal ? doc.body : null);

  class MutationObserver {
    constructor(fn) {
      this.fn = fn;
      this.mine = [];
    }
    observe(target, opts) {
      const o = { target, opts, fn: this.fn };
      this.mine.push(o);
      state.observers.push(o);
    }
    disconnect() {
      state.observers = state.observers.filter((o) => !this.mine.includes(o));
    }
  }

  const sent = [];
  const sandbox = {
    document: doc,
    MutationObserver,
    matchMedia: () => ({ matches: false }),
    setTimeout: (fn, ms) => state.timers.push({ fn, ms, repeat: false }) && state.timers.length,
    clearTimeout: (id) => {
      if (state.timers[id - 1]) state.timers[id - 1].dead = true;
    },
    setInterval: (fn, ms) => state.timers.push({ fn, ms, repeat: true }) && state.timers.length,
    clearInterval: (id) => {
      if (state.timers[id - 1]) state.timers[id - 1].dead = true;
    },
    chrome: { runtime: { sendMessage: (m) => sent.push(m) } },
    innerWidth: 1000,
    innerHeight: 800,
  };
  sandbox.globalThis = sandbox;
  const tick = (repeatOnly = true) => {
    for (const t of [...state.timers]) if (!t.dead && (!repeatOnly || t.repeat)) t.fn();
  };
  const fireTimeouts = () => {
    for (const t of [...state.timers]) if (!t.dead && !t.repeat) {
      t.dead = true;
      t.fn();
    }
  };
  return { sandbox, doc, state, sent, tick, fireTimeouts };
}
