// @ts-nocheck - a hand-made DOM subset for the content script tests (no npm deps, no jsdom).
// It supports what src/content needs: attributes, simple selectors (tag, #id, .class, [attr],
// [attr=value], comma lists, descendant), events with handlers, forms, selects, mutation
// observers and the layout numbers the content script reads.

const VOID = new Set(["input", "br", "img", "hr"]);

function parseSimple(sel) {
  const m = /^([a-z][a-z0-9-]*|\*)?((?:[#.][\w-]+|\[[^\]]+\])*)$/i.exec(sel);
  if (!m) throw new Error(`fake dom: unsupported selector ${sel}`);
  const out = { tag: m[1] && m[1] !== "*" ? m[1].toLowerCase() : null, id: null, classes: [], attrs: [] };
  for (const part of m[2].match(/[#.][\w-]+|\[[^\]]+\]/g) ?? []) {
    if (part[0] === "#") out.id = part.slice(1);
    else if (part[0] === ".") out.classes.push(part.slice(1));
    else {
      const a = /^\[([\w-]+)(?:=(?:"([^"]*)"|'([^']*)'|([^\]]*)))?\]$/.exec(part);
      out.attrs.push({ name: a[1], value: a[2] ?? a[3] ?? a[4] ?? null });
    }
  }
  return out;
}

function matchesSimple(el, s) {
  if (s.tag && el.localName !== s.tag) return false;
  if (s.id && el.getAttribute("id") !== s.id) return false;
  const cls = (el.getAttribute("class") ?? "").split(/\s+/);
  if (!s.classes.every((c) => cls.includes(c))) return false;
  return s.attrs.every((a) => (a.value === null ? el.hasAttribute(a.name) : el.getAttribute(a.name) === a.value));
}

function matchesSelector(el, selector) {
  return selector.split(",").some((alt) => {
    const parts = alt.trim().split(/\s+/).map(parseSimple);
    if (!matchesSimple(el, parts[parts.length - 1])) return false;
    let node = el.parentElement;
    for (let i = parts.length - 2; i >= 0; i -= 1) {
      while (node && !matchesSimple(node, parts[i])) node = node.parentElement;
      if (!node) return false;
      node = node.parentElement;
    }
    return true;
  });
}

export function createWorld(url = "https://shop.test/checkout") {
  const observers = new Set();
  const log = [];
  const notify = (type, target) => {
    for (const o of observers) o.records.push({ type, target });
  };

  class Event {
    constructor(type, init = {}) {
      this.type = type;
      Object.assign(this, { bubbles: false, cancelable: false, composed: false }, init);
      this.defaultPrevented = false;
      this.isTrusted = false;
    }
    preventDefault() {
      if (this.cancelable) this.defaultPrevented = true;
    }
  }
  class MouseEvent extends Event {}
  class InputEvent extends Event {}
  class KeyboardEvent extends Event {}

  class Text {
    constructor(data) {
      this.nodeType = 3;
      this.data = data;
      this.nodeValue = data;
      this.parentNode = null;
    }
    get textContent() {
      return this.data;
    }
  }

  class Element {
    constructor(tag, attrs = {}) {
      this.nodeType = 1;
      this.localName = tag;
      this.tagName = tag.toUpperCase();
      this._attrs = new Map(Object.entries(attrs).map(([k, v]) => [k, String(v)]));
      this.childNodes = [];
      this.parentNode = null;
      this.listeners = new Map();
      this._rect = [0, 0, 100, 30];
      this.shadowRoot = null;
    }
    get parentElement() {
      return this.parentNode && this.parentNode.nodeType === 1 ? this.parentNode : null;
    }
    get children() {
      return this.childNodes.filter((c) => c.nodeType === 1);
    }
    get childElementCount() {
      return this.children.length;
    }
    get previousElementSibling() {
      const sibs = this.parentNode?.children ?? [];
      return sibs[sibs.indexOf(this) - 1] ?? null;
    }
    get nextElementSibling() {
      const sibs = this.parentNode?.children ?? [];
      return sibs[sibs.indexOf(this) + 1] ?? null;
    }
    get isConnected() {
      let n = this;
      while (n.parentNode) n = n.parentNode;
      return n === world.document.documentElement;
    }
    get textContent() {
      return this.childNodes.map((c) => c.textContent).join("");
    }
    get innerText() {
      return this.childNodes.map((c) => (c.nodeType === 3 ? c.data : c.innerText)).join(" ").replace(/\s+/g, " ").trim();
    }
    getAttribute(n) {
      return this._attrs.has(n) ? this._attrs.get(n) : null;
    }
    hasAttribute(n) {
      return this._attrs.has(n);
    }
    setAttribute(n, v) {
      this._attrs.set(n, String(v));
      notify("attributes", this);
    }
    get id() {
      return this.getAttribute("id") ?? "";
    }
    appendChild(c) {
      c.parentNode = this;
      this.childNodes.push(c);
      notify("childList", this);
      return c;
    }
    removeChild(c) {
      this.childNodes.splice(this.childNodes.indexOf(c), 1);
      c.parentNode = null;
      notify("childList", this);
    }
    contains(o) {
      for (let n = o; n; n = n.parentNode) if (n === this) return true;
      return false;
    }
    closest(sel) {
      for (let n = this; n && n.nodeType === 1; n = n.parentElement) if (matchesSelector(n, sel)) return n;
      return null;
    }
    matches(sel) {
      return matchesSelector(this, sel);
    }
    querySelectorAll(sel) {
      const out = [];
      const walk = (n) => {
        for (const c of n.children) {
          if (matchesSelector(c, sel)) out.push(c);
          walk(c);
        }
      };
      walk(this);
      return out;
    }
    querySelector(sel) {
      return this.querySelectorAll(sel)[0] ?? null;
    }
    addEventListener(type, fn) {
      this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn]);
    }
    dispatchEvent(ev) {
      ev.target = this;
      log.push({ type: ev.type, tag: this.localName, id: this.id, trusted: ev.isTrusted });
      for (let n = this; n; n = ev.bubbles ? n.parentNode : null) {
        for (const fn of n.listeners?.get(ev.type) ?? []) fn.call(n, ev);
      }
      return !ev.defaultPrevented;
    }
    click() {
      const ev = new MouseEvent("click", { bubbles: true, cancelable: true });
      const ok = this.dispatchEvent(ev);
      if (ok && this.localName === "input" && ["checkbox", "radio"].includes(this.getAttribute("type"))) {
        this.checked = this.getAttribute("type") === "radio" ? true : !this.checked;
        this.dispatchEvent(new Event("change", { bubbles: true }));
      }
      if (ok && this.localName === "button" && (this.getAttribute("type") ?? "submit") === "submit" && this.form) this.form.requestSubmit(this);
    }
    focus() {
      world.document.activeElement = this;
    }
    scrollIntoView() {
      this.scrolled = true;
    }
    get clientLeft() {
      return 0;
    }
    getBoundingClientRect() {
      const [x, y, width, height] = this._rect;
      return { x, y, width, height, left: x, top: y, right: x + width, bottom: y + height };
    }
    get form() {
      return ["input", "button", "select", "textarea"].includes(this.localName) ? this.closest("form") : null;
    }
    get labels() {
      const id = this.getAttribute("id");
      return id ? world.document.querySelectorAll("label").filter((l) => l.getAttribute("for") === id) : [];
    }
    get isContentEditable() {
      return this.getAttribute("contenteditable") === "true" || this.getAttribute("contenteditable") === "";
    }
    get disabled() {
      return this.hasAttribute("disabled");
    }
    get readOnly() {
      return this.hasAttribute("readonly");
    }
    get href() {
      return this.hasAttribute("href") ? new URL(this.getAttribute("href"), world.location.href).href : "";
    }
    get selectedOptions() {
      return this.localName === "select" ? this.options.filter((o) => o.selected) : [];
    }
    get options() {
      return this.localName === "select" ? this.querySelectorAll("option") : [];
    }
    get multiple() {
      return this.hasAttribute("multiple");
    }
    select() {
      this.selectedAll = true;
    }
  }

  // HTMLInputElement.prototype holds the native value accessor, like a browser.
  class HTMLInputElement extends Element {
    constructor(attrs) {
      super("input", attrs);
      this._value = attrs?.value ?? "";
      this.checked = "checked" in (attrs ?? {});
    }
  }
  Object.defineProperty(HTMLInputElement.prototype, "value", {
    get() {
      return this._value;
    },
    set(v) {
      this._value = String(v);
      this.nativeSets = (this.nativeSets ?? 0) + 1;
    },
  });
  class HTMLTextAreaElement extends Element {
    constructor(attrs) {
      super("textarea", attrs);
      this._value = "";
    }
  }
  Object.defineProperty(HTMLTextAreaElement.prototype, "value", {
    get() {
      return this._value;
    },
    set(v) {
      this._value = String(v);
    },
  });
  class HTMLOptionElement extends Element {
    constructor(attrs) {
      super("option", attrs);
      this.selected = "selected" in (attrs ?? {});
    }
    get value() {
      return this.getAttribute("value") ?? this.textContent;
    }
    get label() {
      return this.textContent;
    }
  }
  class HTMLFormElement extends Element {
    constructor(attrs) {
      super("form", attrs);
      this.submits = [];
    }
    requestSubmit(submitter) {
      this.submits.push(submitter?.getAttribute?.("id") ?? "implicit");
      this.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    }
  }
  class HTMLSelectElement extends Element {
    constructor(attrs) {
      super("select", attrs);
    }
    get value() {
      return this.options.find((o) => o.selected)?.value ?? "";
    }
  }

  function make(tag, attrs = {}) {
    if (tag === "input") return new HTMLInputElement(attrs);
    if (tag === "textarea") return new HTMLTextAreaElement(attrs);
    if (tag === "option") return new HTMLOptionElement(attrs);
    if (tag === "form") return new HTMLFormElement(attrs);
    if (tag === "select") return new HTMLSelectElement(attrs);
    return new Element(tag, attrs);
  }

  /** h("button", {id}, "text", child...) */
  function h(tag, attrs = {}, ...kids) {
    const { rect, ...rest } = attrs;
    const el = make(tag, rest);
    if (rect) el._rect = rect;
    for (const k of kids.flat()) el.appendChild(typeof k === "string" ? new Text(k) : k);
    return el;
  }

  const body = h("body");
  const html = h("html", {}, h("head"), body);
  const doc = {
    documentElement: html,
    body,
    title: "Checkout",
    visibilityState: "visible",
    readyState: "complete",
    activeElement: body,
    createRange: () => ({ selectNodeContents() {} }),
    execCommand: (cmd, _ui, text) => {
      const el = doc.activeElement;
      if (cmd !== "insertText" || !el?.isContentEditable || world.blockExec) return false;
      el.childNodes.length = 0;
      el.appendChild(new Text(text));
      return true;
    },
    elementFromPoint: () => world.hitOverride ?? null,
    getElementById: (id) => html.querySelector(`#${id}`),
    querySelector: (s) => html.querySelector(s),
    querySelectorAll: (s) => html.querySelectorAll(s),
  };
  const world = {
    h,
    Text,
    log,
    document: doc,
    location: new URL(url),
    hitOverride: null,
    blockExec: false,
    observers,
    context: {
      document: doc,
      location: null,
      Event,
      MouseEvent,
      PointerEvent: undefined,
      InputEvent,
      KeyboardEvent,
      MutationObserver: class {
        constructor(cb) {
          this.cb = cb;
          this.records = [];
        }
        observe() {
          observers.add(this);
        }
        disconnect() {
          observers.delete(this);
        }
      },
      WeakRef,
      URL,
      getComputedStyle: () => ({ display: "block", visibility: "visible", opacity: "1", overflow: "visible", clip: "" }),
      getSelection: () => ({ removeAllRanges() {}, addRange() {} }),
      scrollX: 0,
      scrollY: 0,
      innerWidth: 1280,
      innerHeight: 800,
      scrollBy(x, y) {
        world.scrolled.push([x, y]);
      },
      scrollTo() {},
      atob: (s) => Buffer.from(s, "base64").toString("binary"),
      DataTransfer: class {
        constructor() {
          this.files = [];
          this.items = { add: (f) => this.files.push(f) };
        }
      },
      File: class {
        constructor(parts, name, opts) {
          this.name = name;
          this.size = parts.reduce((n, p) => n + p.length, 0);
          this.type = opts?.type ?? "";
        }
      },
      setTimeout,
      console,
    },
    scrolled: [],
  };
  world.context.location = world.location;
  // Mutation records are delivered to the callback on the next tick, like a browser.
  const pump = setInterval(() => {
    for (const o of observers) {
      if (o.records.length) {
        const r = o.records.splice(0);
        o.cb(r);
      }
    }
  }, 1);
  pump.unref?.();
  world.stop = () => clearInterval(pump);
  world.Element = Element;
  return world;
}
