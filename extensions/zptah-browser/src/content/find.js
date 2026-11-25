// Find elements by visible text, name or role. Diacritic-insensitive (Vietnamese users type
// with or without marks). Pure over the model; refs come from the snapshot ref table.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});

  /** @param {string} s */
  const fold = (s) => s.normalize("NFD").replace(/[̀-ͯ]/g, "").replace(/đ/g, "d").replace(/Đ/g, "D").toLowerCase();

  /**
   * @param {any} modelRoot
   * @param {string} query
   * @param {number} max
   * @param {(el: any) => string} refOf
   */
  function findInModel(modelRoot, query, max, refOf) {
    const q = fold(ns.aria.clean(query, 1024));
    /** @type {{ ref: string, role: string, name: string, context: string }[]} */
    const matches = [];
    if (!q) return matches;
    /** @param {any} node @param {any} parent */
    const walk = (node, parent) => {
      if (matches.length >= max || node.t !== "el" || node.hidden) return;
      const role = ns.aria.roleOf(node);
      if (role && node.el) {
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
   * @param {{ generation: number, query: string, max?: number }} args
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

  ns.find = { find, findInModel, fold };
})();
