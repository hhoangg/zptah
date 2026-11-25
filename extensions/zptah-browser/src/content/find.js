// Find elements by visible text, name or role. Diacritic-insensitive (Vietnamese users type
// with or without marks). Pure over the model; refs come from the snapshot ref table.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});

  /** @param {string} s */
  const fold = (s) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/đ/g, "d").replace(/Đ/g, "D").toLowerCase();

  /** ARIA roles a query may start with: `button Buy` finds buttons whose text has "buy". */
  const ROLE_WORDS = new Set([
    "button", "link", "textbox", "checkbox", "radio", "combobox", "listbox", "option", "heading",
    "tab", "menuitem", "switch", "slider", "searchbox", "spinbutton", "img", "dialog", "row", "cell",
  ]);

  /**
   * Splits a role-prefixed query ("button Buy") into its role and the rest ("buy"). A query that
   * does not start with a role word has no role.
   * @param {string} q the folded query
   * @returns {{ role: string, rest: string }}
   */
  function splitRole(q) {
    const space = q.indexOf(" ");
    const first = space === -1 ? q : q.slice(0, space);
    if (!ROLE_WORDS.has(first)) return { role: "", rest: q };
    return { role: first, rest: space === -1 ? "" : q.slice(space + 1).trim() };
  }

  /**
   * @param {any} modelRoot
   * @param {string} query
   * @param {number} max
   * @param {(el: any) => string} refOf
   */
  function findInModel(modelRoot, query, max, refOf) {
    const q = fold(ns.aria.clean(query, 1024));
    if (!q) return [];
    const { role: wanted, rest } = splitRole(q);
    if (wanted) {
      const byRole = search(modelRoot, rest, wanted, max, refOf);
      if (byRole.length > 0) return byRole;
      // "button settings" may also be plain text that happens to start with a role word.
    }
    return search(modelRoot, q, "", max, refOf);
  }

  /**
   * Elements whose name, text, value or placeholder holds `q` (folded); with `wanted` only that role.
   * @param {any} modelRoot @param {string} q @param {string} wanted @param {number} max @param {(el: any) => string} refOf
   */
  function search(modelRoot, q, wanted, max, refOf) {
    /** @type {{ ref: string, role: string, name: string, context: string }[]} */
    const matches = [];
    /** @param {any} node @param {any} parent */
    const walk = (node, parent) => {
      if (matches.length >= max || node.t !== "el" || node.hidden) return;
      const role = ns.aria.roleOf(node);
      if (role && node.el && (!wanted || role === wanted)) {
        const name = ns.aria.nameOf(node, role);
        const own = ns.aria.flatText(node, 400);
        const value = node.value && !ns.aria.isSensitiveField(node) ? String(node.value) : "";
        const hay = fold(`${name} ${own} ${value} ${node.attrs?.placeholder ?? ""}`);
        const useful = ns.aria.isInteractive(role) || role === "heading" || name !== "";
        if (useful && hay.includes(q)) {
          const ctx = parent ? ns.aria.flatText(parent, 300) : own;
          matches.push({ ref: refOf(node.el), role, name: ns.aria.clean(name || own, 200), context: ns.aria.clean(ctx, 300) });
        }
      }
      for (const c of node.children ?? []) walk(c, role ? node : parent);
    };
    walk(modelRoot, null);
    return matches;
  }

  /**
   * @param {{ generation: number, query: string, max?: number, reset?: boolean }} args
   * @param {any} [env]
   */
  function find(args, env) {
    const doc = env?.doc ?? root.document;
    const st = ns.snapshot.state;
    if (args.reset === true) ns.snapshot.beginGeneration(args.generation);
    else if (st.generation !== args.generation) return { stale: true };
    const model = ns.snapshot.domToModel(doc.body ?? doc.documentElement, env);
    const matches = model ? findInModel(model, args.query, args.max ?? 20, ns.snapshot.refFor) : [];
    return { matches, url: String(root.location?.href ?? ""), title: String(doc.title ?? "") };
  }

  ns.find = { find, findInModel, fold, splitRole };
})();
