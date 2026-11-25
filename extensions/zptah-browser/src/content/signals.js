// Signals for one element (spec 7.2), gathered from the DOM. Everything here is raw: field kinds,
// amounts, risk categories and the digest are computed in the worker from the shared lexicon, so
// one implementation decides. Field values are never read (only whether a field is filled).
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});

  const MENU_SELECTOR = "nav, footer, [role=menu], [role=menubar], [role=navigation], [role=contentinfo]";
  const DIALOG_SELECTOR = "dialog[open], [role=dialog], [role=alertdialog], [aria-modal=true]";
  const BUTTONS = "button, [role=button], input[type=submit], input[type=button], input[type=image]";
  const SKIP_INPUT = new Set(["hidden", "submit", "button", "image", "reset"]);
  const PRIMARY_HINT = /primary|confirm|cta|danger|destructive|accept|agree|submit|ok\b/i;
  const VISIBILITY_HINT = /visibility|audience|privacy|who can|quyền|hiển thị|đối tượng/i;
  const RECIPIENT_HINT = /^(to|cc|bcc)\b|recipient|người nhận|đến:/i;

  /** Strings are cut and made well formed (the digest needs that). @param {unknown} s @param {number} max */
  function clip(s, max) {
    const t = ns.aria.clean(s, max);
    return typeof t.toWellFormed === "function" ? t.toWellFormed() : t;
  }

  /** @param {any} el @param {string} name */
  const attr = (el, name) => {
    const v = el?.getAttribute?.(name);
    return v === null || v === undefined ? undefined : String(v);
  };

  /** @param {any} el */
  const textOf = (el) => String(el?.innerText ?? el?.textContent ?? "");

  /** Accessible name and role of one element through the snapshot's own rules. @param {any} el */
  function describe(el) {
    const model = ns.snapshot.domToModel(el);
    if (!model) return { role: "", name: "" };
    const role = ns.aria.roleOf(model);
    return { role, name: ns.aria.nameOf(model, role) || "" };
  }

  /** @param {any} el */
  function labelOf(el) {
    const out = [];
    for (const l of el.labels ?? []) out.push(textOf(l));
    if (out.length === 0) {
      const wrap = el.closest?.("label");
      if (wrap) out.push(textOf(wrap));
    }
    if (out.length === 0) {
      const by = attr(el, "aria-labelledby");
      if (by) for (const id of by.split(/\s+/)) out.push(textOf(root.document.getElementById?.(id)));
    }
    return out.join(" ");
  }

  /** @param {any} el */
  function dialogOf(el) {
    return el.closest?.(DIALOG_SELECTOR) ?? null;
  }

  /** Ids and classes of the dialog and a few ancestors, for consent-manager detection. @param {any} dialog */
  function markersOf(dialog) {
    const parts = [];
    let n = dialog;
    for (let i = 0; n && i < 4; i += 1) {
      const tag = String(n.localName ?? "").toLowerCase();
      if (tag === "body" || tag === "html") break;
      parts.push(attr(n, "id") ?? "", attr(n, "class") ?? "");
      n = n.parentElement;
    }
    return clip(parts.join(" ").toLowerCase(), 400);
  }

  /** The dialog's primary buttons other than `el` (by class, id, type, autofocus). @param {any} dialog @param {any} el */
  function primaryButtons(dialog, el) {
    /** @type {string[]} */
    const names = [];
    for (const b of dialog.querySelectorAll?.(BUTTONS) ?? []) {
      if (b === el || b.contains?.(el)) continue;
      const hint = `${attr(b, "class") ?? ""} ${attr(b, "id") ?? ""} ${attr(b, "data-primary") ?? ""}`;
      const type = (attr(b, "type") ?? "").toLowerCase();
      const primary = PRIMARY_HINT.test(hint) || type === "submit" || b.hasAttribute?.("autofocus");
      if (!primary) continue;
      const d = describe(b);
      const name = clip(d.name || textOf(b), 200);
      if (name) names.push(name);
      if (names.length >= 5) break;
    }
    return names;
  }

  /** The nearest heading and the text around an element (its container's, or its siblings'). @param {any} el */
  function nearbyOf(el) {
    const box = el.closest?.("form, dialog, [role=dialog], section, article, main, li, tr") ?? el.parentElement;
    /** @type {string[]} */
    const parts = [];
    const heading = box?.querySelector?.("h1, h2, h3, legend");
    if (heading) parts.push(clip(textOf(heading), 100));
    const parent = el.parentElement;
    const around = parent ? clip(textOf(parent), 1000) : "";
    if (around && around.length <= 400) parts.push(around);
    else {
      if (el.previousElementSibling) parts.push(clip(textOf(el.previousElementSibling), 150));
      if (el.nextElementSibling) parts.push(clip(textOf(el.nextElementSibling), 150));
    }
    const kept = parts.filter((p, i) => p && !parts.some((q, j) => j !== i && q.length > p.length && q.includes(p)));
    return clip([...new Set(kept)].join(" "), 400);
  }

  /** @param {any} el @param {string} name @param {string} base */
  function absolute(el, name, base) {
    const v = attr(el, name);
    if (v === undefined) return undefined;
    try {
      return new URL(v, base).href;
    } catch {
      return v;
    }
  }

  /** @param {any} el */
  function isSubmit(el) {
    const tag = String(el.localName ?? "").toLowerCase();
    const type = (attr(el, "type") ?? "").toLowerCase();
    if (tag === "input") return type === "submit" || type === "image";
    if (tag === "button") return Boolean(el.form) && (type === "" || type === "submit");
    return false;
  }

  /** The element signals of 7.2 minus what needs the lexicon (amounts). @param {any} el */
  function elementSignals(el) {
    const href0 = String(root.location?.href ?? "");
    const { role, name } = describe(el);
    const tag = String(el.localName ?? "").toLowerCase();
    const dialog = dialogOf(el);
    const form = el.form ?? el.closest?.("form") ?? null;
    /** @type {Record<string, any>} */
    const out = {
      role: clip(role, 64),
      name: clip(name, 200),
      text: clip(textOf(el), 200),
      tag,
      is_submit: isSubmit(el),
      in_dialog: dialog !== null,
      dialog_has_primary: false,
      dialog_primary_names: [],
      in_landmark_menu: Boolean(el.closest?.(MENU_SELECTOR)),
      nearby_text: nearbyOf(el),
    };
    const set = (/** @type {string} */ key, /** @type {string | undefined} */ v, max = 200) => {
      if (v !== undefined && v !== "") out[key] = clip(v, max);
    };
    set("type", tag === "input" || tag === "button" ? (attr(el, "type") ?? "").toLowerCase() : undefined, 64);
    set("autocomplete", attr(el, "autocomplete"), 128);
    set("input_name", attr(el, "name"));
    set("input_id", attr(el, "id"));
    set("placeholder", attr(el, "placeholder"));
    set("label", labelOf(el));
    if (tag === "a" || attr(el, "href") !== undefined) set("href", absolute(el, "href", href0), 8192);
    set("target", attr(el, "target"), 64);
    const action = attr(el, "formaction") ?? (form ? attr(form, "action") ?? "" : undefined);
    if (form || attr(el, "formaction") !== undefined) {
      try {
        out.form_action = clip(new URL(action ?? "", href0).href, 8192);
      } catch {
        out.form_action = clip(action ?? "", 8192);
      }
      out.form_method = clip((attr(el, "formmethod") ?? (form ? attr(form, "method") : undefined) ?? "get").toLowerCase(), 16);
    }
    if (dialog) {
      set("dialog_text", textOf(dialog), 400);
      const names = primaryButtons(dialog, el);
      out.dialog_has_primary = names.length > 0;
      out.dialog_primary_names = names;
      set("dialog_markers", markersOf(dialog), 400);
    }
    return out;
  }

  /** Field attributes for the lexicon's field classifier. @param {any} f */
  function fieldAttrs(f) {
    const ml = Number(attr(f, "maxlength"));
    return {
      type: (attr(f, "type") ?? "").toLowerCase(),
      autocomplete: attr(f, "autocomplete") ?? "",
      name: attr(f, "name") ?? "",
      id: attr(f, "id") ?? "",
      label: clip(labelOf(f) || attr(f, "aria-label") || "", 200),
      placeholder: clip(attr(f, "placeholder") ?? "", 200),
      inputmode: attr(f, "inputmode") ?? "",
      maxlength: Number.isFinite(ml) && ml > 0 ? ml : null,
      nearby_text: clip(textOf(f.parentElement), 120),
    };
  }

  /** @param {any} f */
  function isFilled(f) {
    const type = (attr(f, "type") ?? "").toLowerCase();
    if (type === "checkbox" || type === "radio") return f.checked === true;
    if (type === "file") return (f.files?.length ?? 0) > 0;
    return String(f.value ?? "").length > 0;
  }

  /** @param {any} box */
  function recipientsOf(box) {
    /** @type {string[]} */
    const out = [];
    const add = (/** @type {string} */ v) => {
      for (const part of v.split(/[,;]/)) {
        const t = clip(part, 200);
        if (t && !out.includes(t) && out.length < 20) out.push(t);
      }
    };
    for (const chip of box.querySelectorAll?.("[email], [data-email], [data-hovercard-id]") ?? []) {
      add(attr(chip, "email") ?? attr(chip, "data-email") ?? attr(chip, "data-hovercard-id") ?? "");
    }
    for (const input of box.querySelectorAll?.("input[type=email], input[name], input[aria-label]") ?? []) {
      const hint = `${attr(input, "name") ?? ""} ${attr(input, "aria-label") ?? ""} ${(attr(input, "type") ?? "").toLowerCase() === "email" ? "to" : ""}`;
      if (RECIPIENT_HINT.test(hint.trim()) && String(input.value ?? "") !== "") add(String(input.value));
    }
    return out;
  }

  /** @param {any} box */
  function visibilityOf(box) {
    for (const c of box.querySelectorAll?.("select, [role=combobox], button[aria-haspopup], [role=radiogroup]") ?? []) {
      const hint = `${attr(c, "aria-label") ?? ""} ${attr(c, "name") ?? ""} ${labelOf(c)}`;
      if (!VISIBILITY_HINT.test(hint)) continue;
      const sel = c.selectedOptions?.[0];
      return clip(sel ? textOf(sel) : textOf(c), 100);
    }
    return "";
  }

  /** The form around an element (or the dialog or document body). @param {any} el */
  function formSignals(el) {
    const box = el.form ?? el.closest?.("form") ?? dialogOf(el) ?? root.document?.body ?? el.ownerDocument?.body;
    /** @type {any[]} */
    const fields = [];
    for (const f of box?.querySelectorAll?.("input, textarea, select") ?? []) {
      if (SKIP_INPUT.has((attr(f, "type") ?? "").toLowerCase())) continue;
      if (fields.length >= 60) break;
      fields.push({ attrs: fieldAttrs(f), filled: isFilled(f) });
    }
    const title = root.document?.querySelector?.("main h1, main h2, [role=main] h1, [role=main] h2");
    return {
      fields,
      recipients: box ? recipientsOf(box) : [],
      conversation_title: title ? clip(textOf(title), 200) : "",
      visibility: box ? visibilityOf(box) : "",
    };
  }

  /** @param {any} el */
  function isPassword(el) {
    const tag = String(el?.localName ?? "").toLowerCase();
    if (tag !== "input") return false;
    const type = (attr(el, "type") ?? "").toLowerCase();
    return type === "password" || /current-password|new-password/i.test(attr(el, "autocomplete") ?? "");
  }

  /** Page-level context for the classifier. */
  function pageContext() {
    const doc = root.document;
    const h1 = doc?.querySelector?.("h1");
    return {
      lang: String(doc?.documentElement?.lang ?? ""),
      sample: clip(textOf(doc?.body), 2000),
      heading: h1 ? clip(textOf(h1), 200) : "",
      title: clip(doc?.title ?? "", 200),
      url: String(root.location?.href ?? ""),
    };
  }

  /**
   * The default submit control of a form, for `type` with submit and Enter in a form.
   * @param {any} el
   */
  function submitControlOf(el) {
    const form = el.form ?? el.closest?.("form");
    if (!form) return null;
    for (const b of form.querySelectorAll?.("button, input[type=submit], input[type=image]") ?? []) if (isSubmit(b)) return b;
    return null;
  }

  /**
   * Everything the worker needs from one element.
   * @param {any} el
   */
  function gather(el) {
    const submit = submitControlOf(el);
    return {
      element: elementSignals(el),
      form: formSignals(el),
      page: pageContext(),
      password: isPassword(el),
      field: ["input", "textarea", "select"].includes(String(el.localName ?? "").toLowerCase()) ? fieldAttrs(el) : null,
      submit: submit ? { element: elementSignals(submit), form: formSignals(submit) } : null,
    };
  }

  ns.signals = { gather, elementSignals, formSignals, isPassword, describe, submitControlOf, attr, clip, textOf };
})();
