// Accessibility snapshot in the Playwright YAML dialect: DOM to a plain model, then a pure
// renderer. Refs are `e<N>` per generation; the worker prefixes `f<frame>` for child frames.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});
  const SLOT = "\u0000";
  const SKIP_TAGS = new Set(["script", "style", "noscript", "template", "head", "meta", "link", "title", "base"]);
  const KEEP_ATTRS = [
    "role", "aria-label", "aria-hidden", "aria-expanded", "aria-checked", "aria-selected", "aria-pressed",
    "aria-disabled", "aria-current", "aria-level", "aria-haspopup", "href", "type", "value", "placeholder",
    "alt", "title", "id", "name", "src", "autocomplete", "disabled", "checked", "required", "readonly",
    "multiple", "selected", "open", "list", "size", "for", "maxlength", "inputmode",
  ];
  const MAX_NODES = 30000;
  const MAX_DEPTH = 300;

  /** Per-generation ref table. Survives re-injection because it lives on the namespace. */
  const state = (ns.refs ??= { generation: -1, next: 1, byRef: new Map(), byEl: new WeakMap() });

  /** @param {number} generation */
  function beginGeneration(generation) {
    state.generation = generation;
    state.next = 1;
    state.byRef = new Map();
    state.byEl = new WeakMap();
  }

  /** @param {object} el @returns {string} */
  function refFor(el) {
    const known = state.byEl.get(el);
    if (known) return known;
    const ref = `e${state.next}`;
    state.next += 1;
    state.byEl.set(el, ref);
    state.byRef.set(ref, new WeakRef(el));
    return ref;
  }

  /** @param {string} ref @returns {any} */
  function resolveRef(ref) {
    const el = state.byRef.get(ref)?.deref();
    if (!el || el.isConnected === false) return null;
    return el;
  }

  // ---- DOM to model ----

  /**
   * @param {any} el
   * @param {any} env { getComputedStyle, scrollX, scrollY, doc }
   * @param {{ nodes: number }} budget
   * @param {number} depth
   * @param {boolean} insideSectioning
   * @returns {any}
   */
  function convert(el, env, budget, depth, insideSectioning) {
    const tag = String(el.localName ?? el.tagName ?? "").toLowerCase();
    if (SKIP_TAGS.has(tag) || budget.nodes <= 0) return null;
    if (tag === "input" && String(el.getAttribute?.("type") ?? "").toLowerCase() === "hidden") return null;
    budget.nodes -= 1;
    /** @type {Record<string, string>} */
    const attrs = {};
    for (const name of KEEP_ATTRS) {
      const v = el.getAttribute?.(name);
      if (v !== null && v !== undefined) attrs[name] = String(v);
    }
    if (tag === "a" && attrs.href !== undefined && typeof el.href === "string") attrs.href = el.href;
    if (tag === "img" && typeof el.currentSrc === "string" && el.currentSrc) attrs.src = el.currentSrc;
    if (tag === "iframe" && typeof el.src === "string") attrs.src = el.src;
    let style = /** @type {any} */ ({});
    let rect = /** @type {any} */ ({ width: 1, height: 1, right: 1, bottom: 1 });
    try {
      style = env.getComputedStyle(el);
      rect = el.getBoundingClientRect();
    } catch {
      // detached or unsupported: treat as visible
    }
    const kids = [...(el.shadowRoot?.childNodes ?? []), ...(el.childNodes ?? [])];
    const hidden = ns.aria.isHidden({
      display: style.display,
      visibility: style.visibility,
      opacity: style.opacity,
      ariaHidden: attrs["aria-hidden"],
      hiddenAttr: el.hasAttribute?.("hidden") === true && style.display !== "block",
      width: rect.width,
      height: rect.height,
      right: rect.right + (env.scrollX ?? 0),
      bottom: rect.bottom + (env.scrollY ?? 0),
      overflowHidden: style.overflow === "hidden" || style.clip === "rect(0px, 0px, 0px, 0px)",
      hasChildren: kids.length > 0,
      skipSize: tag === "option" || tag === "optgroup" || tag === "area" || style.display === "contents",
    });
    /** @type {any} */
    const node = { t: "el", tag, attrs, hidden, children: [], el, insideSectioning };
    if (tag === "input" || tag === "textarea" || tag === "select") {
      const type = String(attrs.type ?? "").toLowerCase();
      node.value = tag === "select" ? selectedText(el) : typeof el.value === "string" ? el.value : "";
      if (type === "checkbox" || type === "radio") node.checked = el.checked === true ? (el.indeterminate ? "mixed" : true) : false;
      node.labelText = labelTextOf(el, env);
    } else if (el.hasAttribute?.("aria-labelledby")) {
      node.labelledby = idrefText(el, env);
    }
    if (el.hasAttribute?.("aria-labelledby") && !node.labelledby) node.labelledby = idrefText(el, env);
    if (hidden) {
      // Children are not read; remember only whether anything was there, for the dropped count.
      node.hadContent = String(el.textContent ?? "").trim() !== "" || Boolean(el.querySelector?.("a,button,input,select,textarea,img"));
      return node;
    }
    if (tag === "iframe" || depth > MAX_DEPTH) return node;
    const inner = insideSectioning || tag === "article" || tag === "section" || tag === "main" || tag === "nav" || tag === "aside";
    for (const child of kids) {
      if (child.nodeType === 3) {
        node.children.push({ t: "text", text: String(child.data ?? child.nodeValue ?? "") });
      } else if (child.nodeType === 1) {
        const c = convert(child, env, budget, depth + 1, inner);
        if (c) node.children.push(c);
      }
    }
    return node;
  }

  /** @param {any} el */
  function selectedText(el) {
    const sel = el.selectedOptions?.[0] ?? el.options?.[el.selectedIndex];
    return sel ? String(sel.textContent ?? "") : "";
  }

  /** @param {any} el @param {any} env */
  function idrefText(el, env) {
    const ids = String(el.getAttribute("aria-labelledby") ?? "").split(/\s+/).filter(Boolean);
    return ids.map((id) => String(env.doc?.getElementById?.(id)?.textContent ?? "")).join(" ");
  }

  /** @param {any} el @param {any} env */
  function labelTextOf(el, env) {
    const out = [];
    for (const l of el.labels ?? []) out.push(String(l.textContent ?? ""));
    if (out.length === 0 && el.id && env.doc?.querySelectorAll) {
      for (const l of env.doc.querySelectorAll("label")) if (l.htmlFor === el.id) out.push(String(l.textContent ?? ""));
    }
    return out.join(" ");
  }

  /** @param {any} el @param {any} [env] */
  function domToModel(el, env) {
    const e = env ?? { getComputedStyle: root.getComputedStyle.bind(root), scrollX: root.scrollX, scrollY: root.scrollY, doc: root.document };
    return convert(el, e, { nodes: MAX_NODES }, 0, false);
  }

  // ---- model to entries ----

  /** @param {any} node @returns {boolean} */
  function hasContent(node) {
    if (node.t === "text") return String(node.text).trim() !== "";
    if (node.hidden && node.hadContent !== undefined) return node.hadContent;
    const role = ns.aria.roleOf(node);
    if (ns.aria.isInteractive(role) || node.tag === "img") return true;
    return (node.children ?? []).some(hasContent);
  }

  /**
   * @typedef {object} Entry
   * @property {string} [text]
   * @property {string} [role]
   * @property {string} [name]
   * @property {string[]} [attrs]
   * @property {string} [url]
   * @property {string} [value]
   * @property {any} [el]
   * @property {number} [slot]
   * @property {string} [src]
   * @property {Entry[]} [children]
   */

  /**
   * @param {any} node
   * @param {{ dropped: number, depth: number, iframes: { src: string }[] }} ctx
   * @param {number} level
   * @returns {Entry[]}
   */
  function build(node, ctx, level) {
    if (node.t === "text") {
      const t = ns.aria.clean(node.text, 5000);
      return t ? [{ text: t }] : [];
    }
    if (node.hidden) {
      if (hasContent(node)) ctx.dropped += 1;
      return [];
    }
    const role = ns.aria.roleOf(node);
    const fromContent = ns.aria.nameFromContent(role);
    /** @type {Entry[]} */
    let kids = [];
    if (!role || (!fromContent && level < ctx.depth)) {
      for (const c of node.children ?? []) kids.push(...build(c, ctx, role ? level + 1 : level));
    } else if (!fromContent) {
      // depth limit reached: nothing below
    } else if (role === "button" || role === "link" || role === "heading") {
      // The name already holds the text; nested images and controls are skipped.
    } else {
      for (const c of node.children ?? []) kids.push(...build(c, ctx, level + 1));
    }
    if (!role) return mergeText(kids);
    /** @type {Entry} */
    const entry = { role, name: ns.aria.nameOf(node, role), attrs: [], el: node.el, children: mergeText(kids) };
    const a = node.attrs ?? {};
    if (role === "heading") entry.attrs?.push(`level=${node.tag?.match(/^h([1-6])$/) ? node.tag[1] : Number(a["aria-level"]) || 2}`);
    if (node.checked === true || a["aria-checked"] === "true") entry.attrs?.push("checked");
    else if (node.checked === "mixed" || a["aria-checked"] === "mixed") entry.attrs?.push("checked=mixed");
    if (a.disabled !== undefined || a["aria-disabled"] === "true") entry.attrs?.push("disabled");
    if (a["aria-expanded"] === "true") entry.attrs?.push("expanded");
    if (a["aria-selected"] === "true" || (node.tag === "option" && a.selected !== undefined)) entry.attrs?.push("selected");
    if (a["aria-pressed"] === "true") entry.attrs?.push("pressed");
    if (role === "link" && a.href !== undefined) entry.url = a.href;
    if (node.tag === "input" || node.tag === "textarea" || node.tag === "select") {
      const type = String(a.type ?? "").toLowerCase();
      if (role !== "checkbox" && role !== "radio" && role !== "button" && node.value) {
        entry.value = ns.aria.isSensitiveField(node) ? "<filled>" : ns.aria.clean(node.value, 500);
      } else if (type === "password" && node.value) entry.value = "<filled>";
    }
    if (node.tag === "iframe") {
      entry.slot = ctx.iframes.length;
      entry.src = a.src ?? "";
      ctx.iframes.push({ src: a.src ?? "" });
    }
    return [entry];
  }

  /** @param {Entry[]} list @returns {Entry[]} */
  function mergeText(list) {
    /** @type {Entry[]} */
    const out = [];
    for (const e of list) {
      const last = out[out.length - 1];
      if (e.text !== undefined && last && last.text !== undefined) last.text = `${last.text} ${e.text}`;
      else out.push(e);
    }
    return out;
  }

  /** @param {string} t */
  function textLine(t) {
    return /^["'\[\]{}>&*!|%@`#-]|^\s|\s$|: | #/.test(t) ? JSON.stringify(t) : t;
  }

  /** @param {Entry} e */
  function needsRef(e) {
    return Boolean(e.el) && (ns.aria.isInteractive(e.role ?? "") || e.role === "heading" || e.role === "iframe" || Boolean(e.name));
  }

  /**
   * Renders entries as Playwright style YAML lines. `refOf` maps an element to its ref.
   * @param {Entry[]} entries
   * @param {(el: any) => string} refOf
   * @param {string[]} lines
   * @param {string} pad
   */
  function emit(entries, refOf, lines, pad) {
    for (const e of entries) {
      if (e.text !== undefined) {
        lines.push(`${pad}- text: ${textLine(e.text)}`);
        continue;
      }
      let head = `${pad}- ${e.role}`;
      if (e.name) head += ` ${JSON.stringify(e.name)}`;
      for (const a of e.attrs ?? []) head += ` [${a}]`;
      if (needsRef(e)) head += ` [ref=${refOf(e.el)}]`;
      const kids = e.children ?? [];
      const onlyText = kids.length === 1 && kids[0].text !== undefined && !e.url && e.slot === undefined;
      if (e.value !== undefined) head += `: ${textLine(e.value)}`;
      else if (onlyText && kids[0].text !== e.name) head += `: ${textLine(String(kids[0].text))}`;
      const nested = e.url || e.slot !== undefined || (!onlyText && kids.length > 0);
      if (!nested) {
        lines.push(head);
        continue;
      }
      lines.push(`${head}:`);
      if (e.url) lines.push(`${pad}  - /url: ${JSON.stringify(e.url)}`);
      if (e.slot !== undefined) lines.push(`${pad}  ${SLOT}slot:${e.slot}${SLOT}`);
      if (!onlyText) emit(kids, refOf, lines, `${pad}  `);
    }
  }

  /** @param {Entry[]} entries @param {Entry[]} out */
  function flattenInteractive(entries, out) {
    for (const e of entries) {
      if (e.role && ns.aria.isInteractive(e.role)) out.push({ ...e, children: [] });
      else if (e.role === "iframe") out.push(e);
      if (e.children) flattenInteractive(e.children, out);
    }
  }

  /**
   * Pure: model to snapshot text.
   * @param {any} modelRoot
   * @param {{ mode?: string, depth?: number }} opts
   * @param {(el: any) => string} refOf
   */
  function render(modelRoot, opts, refOf) {
    /** @type {{ dropped: number, depth: number, iframes: { src: string }[] }} */
    const ctx = { dropped: 0, depth: opts.depth ?? 1000, iframes: [] };
    let entries = build(modelRoot, ctx, 0);
    let controls = 0;
    const walk = (/** @type {Entry[]} */ list) => {
      for (const e of list) {
        if (e.role && ns.aria.isInteractive(e.role)) controls += 1;
        if (e.children) walk(e.children);
      }
    };
    walk(entries);
    if (opts.mode === "interactive") {
      /** @type {Entry[]} */
      const flat = [];
      flattenInteractive(entries, flat);
      entries = flat;
    }
    /** @type {string[]} */
    const lines = [];
    emit(entries, refOf, lines, "");
    return { text: lines.join("\n"), hidden_dropped: ctx.dropped, controls, iframes: ctx.iframes };
  }

  /**
   * @param {{ generation: number, mode?: string, depth?: number, scope?: string }} args
   * @param {any} [env]
   */
  function snapshot(args, env) {
    const doc = env?.doc ?? root.document;
    let start = doc.body ?? doc.documentElement;
    if (args.scope) {
      if (state.generation !== args.generation) return { stale: true };
      start = resolveRef(args.scope);
      if (!start) return { stale: true };
    } else {
      beginGeneration(args.generation);
    }
    const model = domToModel(start, env);
    const out = render(model ?? { t: "el", tag: "body", attrs: {}, children: [], hidden: false }, args, refFor);
    return { ...out, url: String(root.location?.href ?? ""), title: String(doc.title ?? ""), visibility: doc.visibilityState === "hidden" ? "hidden" : "visible" };
  }

  ns.snapshot = { snapshot, render, domToModel, build, beginGeneration, refFor, resolveRef, state, SLOT, hasContent };
})();
