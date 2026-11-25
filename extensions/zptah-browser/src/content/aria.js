// Roles, names and visibility rules over a plain element model (no DOM access here, so the
// logic is testable without a DOM). Plain script: the build wraps all content files in one IIFE.
// A model node is { t: "el", tag, attrs, hidden, children, el, labelText?, labelledby?, value?,
// checked? } or { t: "text", text }.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});

  /** @type {Record<string, string>} */
  const ROLE_BY_TAG = {
    a: "link", button: "button", select: "combobox", textarea: "textbox", img: "img", ul: "list",
    ol: "list", li: "listitem", nav: "navigation", main: "main", header: "banner", footer: "contentinfo",
    aside: "complementary", form: "form", table: "table", tr: "row", td: "cell", th: "columnheader",
    p: "paragraph", section: "region", article: "article", dialog: "dialog", iframe: "iframe",
    hr: "separator", details: "group", summary: "button", progress: "progressbar", option: "option",
    h1: "heading", h2: "heading", h3: "heading", h4: "heading", h5: "heading", h6: "heading",
    blockquote: "blockquote", pre: "code", code: "code", figure: "figure", figcaption: "caption",
    caption: "caption", thead: "rowgroup", tbody: "rowgroup", tfoot: "rowgroup", fieldset: "group",
    legend: "legend", menu: "list", dl: "list", dt: "term", dd: "definition", meter: "meter",
    output: "status", address: "group", label: "",
  };

  /** @type {Record<string, string>} */
  const INPUT_ROLE = {
    button: "button", submit: "button", reset: "button", image: "button", checkbox: "checkbox",
    radio: "radio", range: "slider", number: "spinbutton", search: "searchbox", email: "textbox",
    tel: "textbox", url: "textbox", text: "textbox", password: "textbox", "": "textbox",
    date: "textbox", "datetime-local": "textbox", month: "textbox", week: "textbox", time: "textbox",
    color: "button", file: "button",
  };

  const INTERACTIVE = new Set([
    "link", "button", "textbox", "searchbox", "checkbox", "radio", "combobox", "slider", "spinbutton",
    "switch", "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "option", "listbox", "treeitem",
  ]);

  /** Roles whose accessible name comes from their content. */
  const NAME_FROM_CONTENT = new Set([
    "link", "button", "heading", "tab", "menuitem", "menuitemcheckbox", "menuitemradio", "option",
    "treeitem", "checkbox", "radio", "switch", "columnheader", "caption", "legend", "term",
  ]);

  /** Roles that only get a name from an explicit label. */
  const LABELLED = new Set(["form", "region", "dialog", "navigation", "main", "banner", "contentinfo", "complementary", "list", "table", "group", "article", "figure"]);

  /** @param {unknown} s @param {number} [max] */
  function clean(s, max = 200) {
    const t = String(s ?? "").replace(/\u0000/g, "").replace(/\s+/g, " ").trim();
    return t.length > max ? `${t.slice(0, max - 1)}…` : t;
  }

  /** @param {any} node @returns {string} */
  function roleOf(node) {
    const attrs = node.attrs ?? {};
    const explicit = clean(attrs.role, 64).split(" ")[0].toLowerCase();
    if (explicit && explicit !== "presentation" && explicit !== "none") return explicit;
    const tag = node.tag;
    if (tag === "input") {
      const type = String(attrs.type ?? "").toLowerCase();
      if (attrs.list && (type === "" || type === "text" || type === "search" || type === "email" || type === "url" || type === "tel")) return "combobox";
      return INPUT_ROLE[type] ?? "textbox";
    }
    if (tag === "a" && attrs.href === undefined) return "";
    if (tag === "select" && (attrs.multiple !== undefined || Number(attrs.size) > 1)) return "listbox";
    if (tag === "section" && !clean(attrs["aria-label"]) && !node.labelledby) return "";
    if ((tag === "header" || tag === "footer") && node.insideSectioning) return "";
    if (tag === "form" && !clean(attrs["aria-label"]) && !node.labelledby) return "";
    if (tag === "td" && node.inGridHeader) return "columnheader";
    if (tag === "img" && attrs.alt === "") return "";
    return ROLE_BY_TAG[tag] ?? "";
  }

  /**
   * Flattened visible text of a node, including the alt text of images.
   * @param {any} node @param {number} [budget]
   * @returns {string}
   */
  function flatText(node, budget = 400) {
    /** @type {string[]} */
    const out = [];
    let size = 0;
    /** @param {any} n */
    const walk = (n) => {
      if (size >= budget) return;
      if (n.t === "text") {
        const c = clean(n.text, budget);
        if (c) {
          out.push(c);
          size += c.length + 1;
        }
        return;
      }
      if (n.hidden) return;
      if (n.tag === "img") {
        const alt = clean(n.attrs?.alt);
        if (alt) {
          out.push(alt);
          size += alt.length + 1;
        }
        return;
      }
      for (const c of n.children ?? []) walk(c);
    };
    walk(node);
    return clean(out.join(" "), budget);
  }

  /** @param {any} node @param {string} role @returns {string} */
  function nameOf(node, role) {
    const a = node.attrs ?? {};
    const label = clean(a["aria-label"]);
    if (label) return label;
    const by = clean(node.labelledby);
    if (by) return by;
    const tag = node.tag;
    if (tag === "input" || tag === "select" || tag === "textarea") {
      const type = String(a.type ?? "").toLowerCase();
      if (tag === "input" && (type === "button" || type === "submit" || type === "reset")) {
        return clean(a.value) || clean(a.title) || (type === "submit" ? "Submit" : type === "reset" ? "Reset" : "");
      }
      if (tag === "input" && type === "image") return clean(a.alt) || clean(a.title);
      return clean(node.labelText) || clean(a.placeholder) || clean(a.title);
    }
    if (tag === "img") return clean(a.alt) || clean(a.title);
    if (NAME_FROM_CONTENT.has(role)) return flatText(node) || clean(a.title);
    if (LABELLED.has(role)) return clean(a.title);
    return clean(a.title);
  }

  /** Password, one-time code, card and similar fields never show their value. */
  /** @param {any} node */
  function isSensitiveField(node) {
    const a = node.attrs ?? {};
    const type = String(a.type ?? "").toLowerCase();
    if (type === "password") return true;
    const auto = String(a.autocomplete ?? "").toLowerCase();
    if (/one-time-code|cc-|current-password|new-password/.test(auto)) return true;
    const hint = `${a.name ?? ""} ${a.id ?? ""}`.toLowerCase();
    return /(^|[^a-z])(otp|passcode|cvv|cvc|ccnum|cardnumber|card-number|card_number|ssn)([^a-z]|$)/.test(hint);
  }

  /**
   * The prompt-injection hygiene rule (spec 6.1): off-screen, transparent, clipped, zero-size or
   * aria-hidden content is dropped and counted.
   * @param {{ display?: string, visibility?: string, opacity?: number | string, ariaHidden?: string,
   *   hiddenAttr?: boolean, width?: number, height?: number, right?: number, bottom?: number,
   *   overflowHidden?: boolean, hasChildren?: boolean, skipSize?: boolean }} f
   */
  function isHidden(f) {
    if (f.hiddenAttr) return true;
    if (f.display === "none") return true;
    if (f.visibility === "hidden" || f.visibility === "collapse") return true;
    if (f.opacity !== undefined && Number(f.opacity) <= 0.01) return true;
    if (f.ariaHidden === "true") return true;
    if (f.skipSize) return false;
    const w = f.width ?? 1;
    const h = f.height ?? 1;
    if (w === 0 && h === 0) return !f.hasChildren;
    if ((f.right ?? 1) <= 0 || (f.bottom ?? 1) <= 0) return true;
    if (w <= 1 && h <= 1 && f.overflowHidden) return true;
    return false;
  }

  ns.aria = { roleOf, nameOf, flatText, clean, isInteractive: (/** @type {string} */ r) => INTERACTIVE.has(r), nameFromContent: (/** @type {string} */ r) => NAME_FROM_CONTENT.has(r), isHidden, isSensitiveField };
})();
