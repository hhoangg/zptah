// Acting inside the page (spec 6.2): resolve and describe an element at prepare time, check that
// it is still the same node at commit, then do the action with untrusted events, the native
// value setter and requestSubmit. Everything runs in the isolated world and never touches page
// scripts. The worker decides about approval and about the debugger; this file only reports what
// it sees (raw signals) and what happened (effects).
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});

  const MOVE_TOLERANCE = 24;
  const SIZE_TOLERANCE = 24;
  const SETTLE_MS = 150;
  const SCROLL_KEYS = new Set(["PageDown", "PageUp", "Home", "End", "ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", " ", "Space"]);
  const MIME = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", pdf: "application/pdf", txt: "text/plain", csv: "text/csv", json: "application/json", doc: "application/msword", zip: "application/zip" };

  /** @type {Map<string, any>} */
  const prepared = (ns.prepared ??= new Map());

  class ActError extends Error {
    /** @param {string} code @param {string} message */
    constructor(code, message) {
      super(message);
      this.code = code;
    }
  }

  /** @param {number} ms */
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  /** @param {any} el */
  const tagOf = (el) => String(el?.localName ?? "").toLowerCase();
  /** @param {any} el @param {string} n */
  const attr = (el, n) => ns.signals.attr(el, n);

  /** Page coordinates of an element, so scrolling does not count as movement. @param {any} el */
  function box(el) {
    const r = el.getBoundingClientRect();
    return { x: r.x + (root.scrollX ?? 0), y: r.y + (root.scrollY ?? 0), w: r.width, h: r.height };
  }

  /** @param {any} el */
  function isEditableHost(el) {
    if (!el) return false;
    const t = tagOf(el);
    return t === "input" || t === "textarea" || t === "select" || el.isContentEditable === true;
  }

  /** A readonly, aria-hidden or zero-size input is a decoy (F6). @param {any} el */
  function isDecoy(el) {
    const r = el.getBoundingClientRect?.();
    return el.readOnly === true || attr(el, "aria-hidden") === "true" || (r && (r.width < 2 || r.height < 2));
  }

  /**
   * The element to type into: the ref itself, or the first usable field inside a wrapper.
   * @param {any} el
   */
  function editableTarget(el) {
    if (isEditableHost(el) && !isDecoy(el)) return el;
    if (!isEditableHost(el)) {
      const inner = [...(el.querySelectorAll?.("input, textarea, [contenteditable=true], [contenteditable='']") ?? [])].find((c) => isEditableHost(c) && !isDecoy(c) && !["hidden", "submit", "button", "checkbox", "radio", "file"].includes((attr(c, "type") ?? "").toLowerCase()));
      if (inner) return inner;
    }
    throw new ActError("E_INVALID", "That element cannot be typed into (it is readonly, hidden or not a field). Take a new snapshot and pick the visible field.");
  }

  /** @param {string} ref @returns {any} */
  function resolve(ref) {
    const el = ns.snapshot.resolveRef(ref);
    if (!el) throw new ActError("E_STALE_REF", "That ref is gone. Take a new snapshot.");
    return el;
  }

  /** @param {any} el */
  function describeEntry(el) {
    return { wr: new WeakRef(el), name: ns.signals.describe(el).name, box: box(el) };
  }

  /**
   * Resolves the refs, scrolls into view and reports the raw signals of the target.
   * @param {{ generation: number, token: string, kind: string, refs: string[], action: any }} a
   */
  function prepare(a) {
    if (ns.snapshot.state.generation !== a.generation) return { stale: true };
    /** @type {any[]} */
    let els = a.refs.map(resolve);
    if (a.kind === "type" || a.kind === "fill") els = els.map(editableTarget);
    const primary = els[0] ?? root.document.activeElement ?? root.document.body;
    primary.scrollIntoView?.({ block: "center", inline: "center", behavior: "instant" });
    const old = prepared.get(a.token);
    old?.observer?.disconnect?.();
    prepared.set(a.token, { kind: a.kind, generation: a.generation, refs: a.refs, entries: els.map(describeEntry), primaryIsActive: els.length === 0 });
    const raw = ns.signals.gather(primary);
    const rawFields = a.kind === "fill" ? els.map((e) => ns.signals.gather(e)) : null;
    return { raw, rawFields, url: String(root.location?.href ?? ""), title: String(root.document.title ?? "") };
  }

  /**
   * Same node, still connected, same name, same place (within a tolerance).
   * @param {string} token
   */
  function sameNodes(token) {
    const rec = prepared.get(token);
    if (!rec) throw new ActError("E_STALE_PREPARE", "The prepared action is gone (the page was reloaded).");
    if (ns.snapshot.state.generation !== rec.generation) throw new ActError("E_STALE_PREPARE", "The page was read again since the prepare.");
    /** @type {any[]} */
    const els = [];
    for (const e of rec.entries) {
      const el = e.wr.deref();
      if (!el || el.isConnected === false) throw new ActError("E_STALE_PREPARE", "The element is gone.");
      if (ns.signals.describe(el).name !== e.name) throw new ActError("E_STALE_PREPARE", "The element's name changed.");
      const b = box(el);
      if (Math.abs(b.x - e.box.x) > MOVE_TOLERANCE || Math.abs(b.y - e.box.y) > MOVE_TOLERANCE || Math.abs(b.w - e.box.w) > SIZE_TOLERANCE || Math.abs(b.h - e.box.h) > SIZE_TOLERANCE) {
        throw new ActError("E_STALE_PREPARE", "The element moved or changed size.");
      }
      els.push(el);
    }
    return { rec, els };
  }

  /** @param {any} v */
  const rawKey = (v) => JSON.stringify(v);

  /** Gathers again and reports it with a stable key for the later checks. @param {{ token: string }} a */
  function revalidate(a) {
    const { rec, els } = sameNodes(a.token);
    const primary = els[0] ?? root.document.activeElement ?? root.document.body;
    const raw = ns.signals.gather(primary);
    const rawFields = rec.kind === "fill" ? els.map((e) => ns.signals.gather(e)) : null;
    return { raw, rawFields, key: rawKey({ raw, rawFields }), url: String(root.location?.href ?? ""), title: String(root.document.title ?? ""), mutations: (rec.mutations ?? 0) - (rec.mark ?? 0) };
  }

  /** The key must still match right before the page is touched. @param {{ token: string, key: string }} a */
  function recheck(a) {
    const { rec, els } = sameNodes(a.token);
    const primary = els[0] ?? root.document.activeElement ?? root.document.body;
    const raw = ns.signals.gather(primary);
    const rawFields = rec.kind === "fill" ? els.map((e) => ns.signals.gather(e)) : null;
    if (rawKey({ raw, rawFields }) !== a.key) throw new ActError("E_STALE_PREPARE", "The page changed between the check and the action.");
    return { rec, els };
  }

  // ---- events ----

  /** @param {string} type @param {any} init */
  function makeEvent(type, init) {
    const C = type.startsWith("pointer") && root.PointerEvent ? root.PointerEvent : root.MouseEvent;
    return new C(type, { bubbles: true, cancelable: true, composed: true, view: root, ...init });
  }

  /** @param {any} el @param {string} type @param {number} x @param {number} y @param {any} [o] */
  function fire(el, type, x, y, o = {}) {
    el.dispatchEvent(makeEvent(type, { clientX: x, clientY: y, button: o.button ?? 0, buttons: o.buttons ?? 0, detail: o.detail ?? 0, ctrlKey: o.ctrl === true, shiftKey: o.shift === true, altKey: o.alt === true, metaKey: o.meta === true, pointerId: 1, pointerType: "mouse", isPrimary: true }));
  }

  /** Centre of an element in viewport coordinates, plus whether something else covers it. @param {any} el */
  function centre(el) {
    const r = el.getBoundingClientRect();
    const x = r.x + r.width / 2;
    const y = r.y + r.height / 2;
    const hit = root.document.elementFromPoint?.(x, y);
    const covered = Boolean(hit) && !(el.contains?.(hit) || hit.contains?.(el) || hit === el);
    return { x, y, covered, coveredBy: covered ? ns.signals.clip(ns.signals.describe(hit).name || tagOf(hit), 80) : "" };
  }

  /** @param {any} el */
  function requireVisibleTarget(el) {
    const c = centre(el);
    if (c.covered) throw new ActError("E_INVALID", `That control is covered by another element (${c.coveredBy || "unnamed"}). Close or move it first.`);
    return c;
  }

  /** @param {string[]} [mods] */
  const modFlags = (mods = []) => ({ ctrl: mods.includes("Control") || mods.includes("Ctrl"), shift: mods.includes("Shift"), alt: mods.includes("Alt"), meta: mods.includes("Meta") || mods.includes("Command") });

  /** @param {any} el @param {any} action */
  function doClick(el, action) {
    const { x, y } = requireVisibleTarget(el);
    const mods = modFlags(action.modifiers);
    const button = action.button === "right" ? 2 : action.button === "middle" ? 1 : 0;
    const buttons = button === 0 ? 1 : button === 1 ? 4 : 2;
    for (const t of ["pointerover", "pointerenter", "mouseover", "mouseenter", "pointermove", "mousemove"]) fire(el, t, x, y, { ...mods });
    const rounds = action.double === true ? 2 : 1;
    for (let i = 1; i <= rounds; i += 1) {
      fire(el, "pointerdown", x, y, { ...mods, button, buttons, detail: i });
      fire(el, "mousedown", x, y, { ...mods, button, buttons, detail: i });
      el.focus?.({ preventScroll: true });
      fire(el, "pointerup", x, y, { ...mods, button, detail: i });
      fire(el, "mouseup", x, y, { ...mods, button, detail: i });
      const plain = button === 0 && !mods.ctrl && !mods.shift && !mods.alt && !mods.meta;
      if (plain) el.click();
      else fire(el, button === 0 ? "click" : button === 1 ? "auxclick" : "contextmenu", x, y, { ...mods, button, detail: i });
    }
    if (rounds === 2) fire(el, "dblclick", x, y, { ...mods, detail: 2 });
  }

  /** @param {any} el */
  function doHover(el) {
    const { x, y } = requireVisibleTarget(el);
    for (const t of ["pointerover", "pointerenter", "mouseover", "mouseenter", "pointermove", "mousemove"]) fire(el, t, x, y);
  }

  /** The prototype's own value setter, so frameworks that wrap `value` still see the change. @param {any} el @param {string} v */
  function nativeSet(el, v) {
    let proto = Object.getPrototypeOf(el);
    while (proto) {
      const d = Object.getOwnPropertyDescriptor(proto, "value");
      if (d?.set) {
        d.set.call(el, v);
        return;
      }
      proto = Object.getPrototypeOf(proto);
    }
    el.value = v;
  }

  /** @param {string} type @param {any} init */
  function inputEvent(type, init) {
    const C = root.InputEvent ?? root.Event;
    return new C(type, { bubbles: true, cancelable: type === "beforeinput", composed: true, ...init });
  }

  /** @param {any} el @param {string} data @param {string} inputType */
  function announce(el, data, inputType) {
    return !el.dispatchEvent(inputEvent("beforeinput", { inputType, data }));
  }

  /** @param {any} el @param {string} text @param {boolean} replace */
  function typeIntoField(el, text, replace) {
    el.focus?.({ preventScroll: true });
    if (replace && typeof el.select === "function") el.select();
    announce(el, text, replace ? "insertReplacementText" : "insertText");
    nativeSet(el, replace ? text : String(el.value ?? "") + text);
    el.dispatchEvent(inputEvent("input", { inputType: replace ? "insertReplacementText" : "insertText", data: text }));
    el.dispatchEvent(new root.Event("change", { bubbles: true }));
  }

  /** @param {any} el @param {string} ch */
  function keyOnce(el, ch) {
    const init = { key: ch, bubbles: true, cancelable: true, composed: true };
    el.dispatchEvent(new root.KeyboardEvent("keydown", init));
    el.dispatchEvent(new root.KeyboardEvent("keypress", init));
    announce(el, ch, "insertText");
    nativeSet(el, String(el.value ?? "") + ch);
    el.dispatchEvent(inputEvent("input", { inputType: "insertText", data: ch }));
    el.dispatchEvent(new root.KeyboardEvent("keyup", init));
  }

  /** @param {any} el @param {string} text @returns {{ ok: boolean, needsDebugger?: boolean }} */
  function typeIntoEditable(el, text) {
    el.focus?.({ preventScroll: true });
    if (announce(el, text, "insertText")) return { ok: false, needsDebugger: true };
    const sel = root.getSelection?.();
    if (sel && root.document.createRange) {
      const range = root.document.createRange();
      range.selectNodeContents(el);
      sel.removeAllRanges();
      sel.addRange(range);
    }
    const done = root.document.execCommand?.("insertText", false, text) === true;
    return { ok: done, needsDebugger: !done };
  }

  /** @param {any} el */
  function requestSubmit(el) {
    const form = el.form ?? el.closest?.("form");
    if (!form) return false;
    if (typeof form.requestSubmit === "function") form.requestSubmit();
    else form.submit();
    return true;
  }

  /** @param {any} el @param {string} key */
  function pressEnterEvents(el, key) {
    const init = { key, code: key, bubbles: true, cancelable: true, composed: true };
    el.dispatchEvent(new root.KeyboardEvent("keydown", init));
    el.dispatchEvent(new root.KeyboardEvent("keypress", init));
    el.dispatchEvent(new root.KeyboardEvent("keyup", init));
  }

  /** @param {any} el @param {string} value */
  function chooseOption(el, value) {
    const wanted = ns.find.fold(value);
    const options = [...(el.options ?? [])];
    const hit = options.find((o) => o.value === value) ?? options.find((o) => ns.find.fold(String(o.textContent ?? "").trim()) === wanted) ?? options.find((o) => ns.find.fold(String(o.label ?? "")) === wanted);
    return hit ?? null;
  }

  /** @param {any} el @param {string[]} values */
  function doSelect(el, values) {
    if (tagOf(el) !== "select") throw new ActError("E_INVALID", "That element is not a select list. Click it, then click the option.");
    const picks = values.map((v) => {
      const o = chooseOption(el, v);
      if (!o) throw new ActError("E_INVALID", `No option matches ${JSON.stringify(v.slice(0, 60))}.`);
      return o;
    });
    const many = el.multiple === true;
    for (const o of el.options ?? []) o.selected = false;
    for (const o of many ? picks : picks.slice(0, 1)) o.selected = true;
    el.dispatchEvent(inputEvent("input", { inputType: "insertReplacementText" }));
    el.dispatchEvent(new root.Event("change", { bubbles: true }));
    return picks.every((o) => o.selected === true);
  }

  /** @param {any} el @param {any} f one fill field */
  function doFillOne(el, f) {
    const type = (attr(el, "type") ?? "").toLowerCase();
    if (f.type === "checkbox" || f.type === "radio") {
      const want = f.type === "radio" ? true : /^(true|1|on|yes|checked)$/i.test(f.value);
      if ((el.checked === true) !== want) {
        requireVisibleTarget(el);
        el.click();
      }
      return el.checked === want;
    }
    if (f.type === "combobox" && tagOf(el) === "select") return doSelect(el, [f.value]);
    if (f.type === "slider" || type === "range") {
      nativeSet(el, f.value);
      el.dispatchEvent(inputEvent("input", { inputType: "insertReplacementText" }));
      el.dispatchEvent(new root.Event("change", { bubbles: true }));
      return String(el.value) === f.value;
    }
    if (el.isContentEditable === true) return typeIntoEditable(el, f.value).ok;
    typeIntoField(el, f.value, true);
    return true;
  }

  /** @param {any} el @param {string} key */
  function doPress(el, key) {
    const k = key.split("+").pop() ?? key;
    const mods = key.split("+").slice(0, -1);
    const target = el ?? root.document.activeElement ?? root.document.body;
    if (mods.length === 0 && SCROLL_KEYS.has(k)) {
      const page = root.innerHeight ?? 800;
      const by = { PageDown: [0, page * 0.9], PageUp: [0, -page * 0.9], ArrowDown: [0, 40], ArrowUp: [0, -40], ArrowLeft: [-40, 0], ArrowRight: [40, 0], " ": [0, page * 0.9], Space: [0, page * 0.9] }[k];
      if (k === "Home") root.scrollTo?.(root.scrollX ?? 0, 0);
      else if (k === "End") root.scrollTo?.(root.scrollX ?? 0, root.document.documentElement?.scrollHeight ?? 1e6);
      else if (by) root.scrollBy?.(by[0], by[1]);
      return { handled: true };
    }
    if (k === "Enter" && mods.length === 0) {
      if (requestSubmit(target)) return { handled: true, submitted: true };
      pressEnterEvents(target, "Enter");
      return { handled: true, submitted: false };
    }
    if (k === "Tab") {
      const list = [...root.document.querySelectorAll("a[href], button, input, select, textarea, [tabindex]")].filter((e) => e.disabled !== true && attr(e, "tabindex") !== "-1" && attr(e, "type") !== "hidden" && !isDecoy(e));
      const i = list.indexOf(target);
      const next = list[(i + (mods.includes("Shift") ? -1 : 1) + list.length) % Math.max(list.length, 1)];
      next?.focus?.({ preventScroll: true });
      return { handled: true };
    }
    return { handled: false };
  }

  /** @param {any} el @param {any} a */
  function doScroll(el, a) {
    if (el && typeof el.scrollBy === "function") el.scrollBy(a.dx, a.dy);
    else root.scrollBy?.(a.dx, a.dy);
  }

  /** @param {string} name */
  const mimeOf = (name) => /** @type {any} */ (MIME)[name.split(".").pop()?.toLowerCase() ?? ""] ?? "application/octet-stream";

  /** @param {string} b64 */
  function bytesOf(b64) {
    const s = root.atob(b64);
    const out = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i += 1) out[i] = s.charCodeAt(i);
    return out;
  }

  /** @param {any} el @param {{ name: string, b64: string }[]} files */
  function doUpload(el, files) {
    if (tagOf(el) !== "input" || (attr(el, "type") ?? "").toLowerCase() !== "file") throw new ActError("E_INVALID", "That element is not a file input.");
    const dt = new root.DataTransfer();
    for (const f of files) dt.items.add(new root.File([bytesOf(f.b64)], f.name, { type: mimeOf(f.name) }));
    el.files = dt.files;
    el.dispatchEvent(inputEvent("input", {}));
    el.dispatchEvent(new root.Event("change", { bubbles: true }));
    return (el.files?.length ?? 0) === files.length;
  }

  /** Counts DOM mutations and notes the address and the focused element from now on. @param {any} rec */
  function startObserving(rec) {
    const doc = root.document;
    rec.observer?.disconnect?.();
    rec.mutations = 0;
    rec.mark = 0;
    rec.before = { url: String(root.location?.href ?? ""), active: doc.activeElement };
    rec.observer = new root.MutationObserver((/** @type {any[]} */ list) => {
      rec.mutations += list.length;
    });
    rec.observer.observe(doc.documentElement ?? doc, { subtree: true, childList: true, attributes: true, characterData: true });
  }

  /** @param {any} rec */
  function finishObserving(rec) {
    return {
      dom_mutations: rec.mutations ?? 0,
      url_changed: String(root.location?.href ?? "") !== rec.before?.url,
      focus_changed: root.document.activeElement !== rec.before?.active,
    };
  }

  /** For the debugger path: start watching before the trusted events go out. @param {{ token: string }} a */
  function observe(a) {
    const rec = prepared.get(a.token);
    if (!rec) throw new ActError("E_STALE_PREPARE", "The prepared action is gone (the page was reloaded).");
    startObserving(rec);
    return { observing: true };
  }

  /** @param {{ token: string }} a */
  function observed(a) {
    const rec = prepared.get(a.token);
    if (!rec) return { dom_mutations: 0, url_changed: true, focus_changed: false };
    const seen = finishObserving(rec);
    rec.mark = rec.mutations;
    return seen;
  }

  /**
   * Does the prepared action with untrusted events. Resolves with the observed effects, or
   * `{ needs_debugger: true }` when nothing was done because the page needs trusted input.
   * @param {{ token: string, key: string, action: any, files?: { name: string, b64: string }[] }} a
   */
  async function perform(a) {
    const { rec, els } = recheck(a);
    const action = a.action;
    const doc = root.document;
    startObserving(rec);
    /** @type {any} */
    const extra = {};
    const el = els[0];
    let valueOk;
    switch (action.kind) {
      case "click":
        doClick(el, action);
        break;
      case "hover":
        doHover(el);
        break;
      case "scroll":
        doScroll(el, action);
        break;
      case "select":
        valueOk = doSelect(el, action.values);
        break;
      case "upload":
        valueOk = doUpload(el, a.files ?? []);
        break;
      case "type": {
        if (el.isContentEditable === true && !isEditableHost(el.parentElement)) {
          const r = typeIntoEditable(el, action.text);
          if (r.needsDebugger) {
            rec.observer?.disconnect?.();
            return { needs_debugger: true };
          }
        } else if (action.slowly === true) {
          el.focus?.({ preventScroll: true });
          if (typeof el.select === "function") el.select();
          nativeSet(el, "");
          for (const ch of action.text) keyOnce(el, ch);
          el.dispatchEvent(new root.Event("change", { bubbles: true }));
        } else {
          typeIntoField(el, action.text, true);
        }
        await sleep(SETTLE_MS);
        valueOk = el.isContentEditable === true ? String(el.textContent ?? "").includes(action.text) : String(el.value ?? "") === action.text;
        if (action.submit === true) {
          if (requestSubmit(el)) extra.submitted = true;
          else {
            pressEnterEvents(el, "Enter");
            extra.submitted = false;
            extra.hint = "This field is not in a form. If nothing changed, prepare the site's search or submit button as its own click.";
          }
        }
        break;
      }
      case "fill": {
        valueOk = true;
        for (let i = 0; i < action.fields.length; i += 1) valueOk = doFillOne(els[i], action.fields[i]) && valueOk;
        break;
      }
      case "press": {
        const r = doPress(a.action.ref ? el : null, action.key);
        if (!r.handled) {
          rec.observer?.disconnect?.();
          return { needs_debugger: true };
        }
        if (r.submitted !== undefined) extra.submitted = r.submitted;
        break;
      }
      default:
        rec.observer?.disconnect?.();
        return { needs_debugger: true };
    }
    await sleep(SETTLE_MS);
    const seen = finishObserving(rec);
    /** @type {any} */
    const effects = {
      method: "content",
      dom_mutations: seen.dom_mutations,
      url_changed: seen.url_changed,
      focus_changed: seen.focus_changed && !["type", "fill", "click"].includes(action.kind),
      ...(valueOk === undefined ? {} : { value_matches: valueOk }),
      ...extra,
    };
    if (valueOk === false) effects.ignored = true;
    rec.mark = rec.mutations;
    return { effects, url: String(root.location?.href ?? ""), title: String(doc.title ?? "") };
  }

  /**
   * Where the debugger should click: the centre of the prepared element(s) in the top frame's
   * viewport, after the same checks as `perform`.
   * @param {{ token: string, key: string, which?: number }} a
   */
  function locate(a) {
    const { els } = recheck(a);
    const el = els[a.which ?? 0] ?? els[0];
    if (!el) throw new ActError("E_INVALID", "There is no element to point at.");
    el.scrollIntoView?.({ block: "center", inline: "center", behavior: "instant" });
    const c = requireVisibleTarget(el);
    let { x, y } = c;
    let w = root;
    while (w !== w.top) {
      const fe = w.frameElement;
      if (!fe) throw new ActError("E_UNSUPPORTED", "Trusted input inside a cross-origin frame is not supported.");
      const r = fe.getBoundingClientRect();
      x += r.x + (fe.clientLeft ?? 0);
      y += r.y + (fe.clientTop ?? 0);
      w = w.parent;
    }
    return { x, y, editable: isEditableHost(el), password: ns.signals.isPassword(el) };
  }

  /** Focuses the prepared field for debugger typing and reports whether it is a password field. @param {{ token: string, key: string, which?: number }} a */
  function focusTarget(a) {
    const { els } = recheck(a);
    const el = els[a.which ?? 0] ?? els[0];
    // The worker classifies the field attributes with the lexicon too (a text field labelled "Password").
    const field = ns.signals.fieldAttrsOf(el);
    if (ns.signals.isPassword(el)) throw new ActError("E_PASSWORD_FIELD", "zptah Browser never types into password fields.");
    el.focus?.({ preventScroll: true });
    return { focused: root.document.activeElement === el, field };
  }

  /** The state of the focused element, for the password check before each key or text. */
  function active() {
    let el = root.document.activeElement;
    while (el?.shadowRoot?.activeElement) el = el.shadowRoot.activeElement;
    return { password: ns.signals.isPassword(el), tag: tagOf(el), editable: el ? isEditableHost(el) : false, field: el ? ns.signals.fieldAttrsOf(el) : null };
  }

  /** @param {{ token: string }} a */
  function release(a) {
    const rec = prepared.get(a.token);
    rec?.observer?.disconnect?.();
    prepared.delete(a.token);
    return { released: Boolean(rec) };
  }

  /** @param {{ token: string }} a */
  function mutationsSince(a) {
    const rec = prepared.get(a.token);
    return { mutations: rec ? (rec.mutations ?? 0) - (rec.mark ?? 0) : 0 };
  }

  /**
   * Where the cursor goes: the prepared element's centre in this frame's viewport and its
   * accessible name (never a value the user typed). @param {{ token: string }} a
   */
  function point(a) {
    const { els } = sameNodes(a.token);
    const el = els[0];
    if (!el) return null;
    const vw = root.innerWidth || 0;
    const vh = root.innerHeight || 0;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return null;
    // Nothing is scrolled for the cursor: an element out of view gets no cursor.
    if (r.bottom < 0 || r.top > vh || r.right < 0 || r.left > vw) return null;
    const text = String(el.getAttribute?.("aria-label") || el.innerText || el.getAttribute?.("title") || el.getAttribute?.("placeholder") || "");
    return { x: r.x + r.width / 2, y: r.y + r.height / 2, name: text.replace(/\s+/g, " ").trim().slice(0, 80) };
  }

  ns.act = { point, prepare, revalidate, perform, locate, focusTarget, active, release, mutationsSince, observe, observed, ActError, prepared };
})();
