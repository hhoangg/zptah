// Reader view: page model to Markdown, main content only. Pure over the element model.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});
  const DROP = new Set(["nav", "footer", "aside", "form", "dialog", "select", "textarea", "button", "input", "svg", "iframe", "video", "audio", "canvas", "object", "embed"]);

  /** @param {any} node @param {string} tag @returns {any} */
  function findTag(node, tag) {
    if (node.t !== "el" || node.hidden) return null;
    if (node.tag === tag || node.attrs?.role === tag) return node;
    for (const c of node.children ?? []) {
      const f = findTag(c, tag);
      if (f) return f;
    }
    return null;
  }

  /** @param {any} node @returns {string} */
  function inline(node) {
    if (node.t === "text") return String(node.text).replace(/\s+/g, " ");
    if (node.hidden || DROP.has(node.tag)) return "";
    const inner = (node.children ?? []).map(inline).join("");
    switch (node.tag) {
      case "a": {
        const text = inner.trim();
        const href = node.attrs?.href;
        return text && href && !href.startsWith("javascript:") ? `[${text}](${href})` : inner;
      }
      case "strong": case "b": return inner.trim() ? `**${inner.trim()}**` : "";
      case "em": case "i": return inner.trim() ? `*${inner.trim()}*` : "";
      case "code": return inner.trim() ? `\`${inner.trim()}\`` : "";
      case "img": return node.attrs?.alt ? `![${ns.aria.clean(node.attrs.alt)}](${node.attrs.src ?? ""})` : "";
      case "br": return "\n";
      default: return inner;
    }
  }

  /** @param {any} node @returns {string} */
  function rawText(node) {
    if (node.t === "text") return String(node.text);
    if (node.hidden) return "";
    return (node.children ?? []).map(rawText).join("");
  }

  /** @param {any} node @param {string[]} out @param {string} indent */
  function block(node, out, indent) {
    if (node.t === "text") {
      const t = inline(node).trim();
      if (t) out.push(`${indent}${t}`, "");
      return;
    }
    if (node.hidden || DROP.has(node.tag)) return;
    const tag = node.tag;
    const h = /^h([1-6])$/.exec(tag);
    if (h) {
      const t = inline(node).replace(/\s+/g, " ").trim();
      if (t) out.push(`${"#".repeat(Number(h[1]))} ${t}`, "");
    } else if (tag === "p" || tag === "figcaption" || tag === "dt" || tag === "dd" || tag === "summary") {
      const t = inline(node).trim();
      if (t) out.push(`${indent}${t}`, "");
    } else if (tag === "ul" || tag === "ol") {
      let n = 0;
      for (const li of node.children ?? []) {
        if (li.t !== "el" || li.hidden || li.tag !== "li") continue;
        n += 1;
        /** @type {string[]} */
        const sub = [];
        const parts = (li.children ?? []).filter((/** @type {any} */ c) => c.t === "el" && (c.tag === "ul" || c.tag === "ol"));
        const text = (li.children ?? []).filter((/** @type {any} */ c) => !parts.includes(c)).map(inline).join("").replace(/\s+/g, " ").trim();
        out.push(`${indent}${tag === "ol" ? `${n}.` : "-"} ${text}`);
        for (const p of parts) block(p, sub, `${indent}  `);
        out.push(...sub.filter((l, i, a) => !(l === "" && i === a.length - 1)));
      }
      out.push("");
    } else if (tag === "pre") {
      out.push("```", rawText(node).replace(/\n+$/, ""), "```", "");
    } else if (tag === "blockquote") {
      /** @type {string[]} */
      const sub = [];
      for (const c of node.children ?? []) block(c, sub, "");
      for (const l of sub) out.push(l === "" ? ">" : `> ${l}`);
      out.push("");
    } else if (tag === "hr") {
      out.push("---", "");
    } else if (tag === "table") {
      /** @type {string[][]} */
      const rows = [];
      const collect = (/** @type {any} */ n) => {
        if (n.t !== "el" || n.hidden) return;
        if (n.tag === "tr") rows.push((n.children ?? []).filter((/** @type {any} */ c) => c.t === "el" && !c.hidden && (c.tag === "td" || c.tag === "th")).map((/** @type {any} */ c) => inline(c).replace(/\s+/g, " ").replace(/\|/g, "\\|").trim()));
        else for (const c of n.children ?? []) collect(c);
      };
      collect(node);
      if (rows.length > 0) {
        const width = Math.max(...rows.map((r) => r.length));
        const pad = (/** @type {string[]} */ r) => `| ${[...r, ...Array(width - r.length).fill("")].join(" | ")} |`;
        out.push(pad(rows[0]), `| ${Array(width).fill("---").join(" | ")} |`, ...rows.slice(1).map(pad), "");
      }
    } else if (tag === "img") {
      const t = inline(node);
      if (t) out.push(t, "");
    } else {
      // Container: block children are emitted on their own, loose inline content as a paragraph.
      let loose = "";
      const flush = () => {
        const t = loose.replace(/[ \t]+/g, " ").trim();
        if (t) out.push(`${indent}${t}`, "");
        loose = "";
      };
      for (const c of node.children ?? []) {
        const isBlock = c.t === "el" && !c.hidden && /^(div|section|article|main|header|ul|ol|p|h[1-6]|pre|blockquote|table|hr|figure|details|dl|li|body)$/.test(c.tag);
        if (isBlock) {
          flush();
          block(c, out, indent);
        } else loose += inline(c);
      }
      flush();
    }
  }

  /**
   * @param {any} modelRoot
   * @returns {string}
   */
  function toMarkdown(modelRoot) {
    const main = findTag(modelRoot, "main") ?? findTag(modelRoot, "article") ?? modelRoot;
    /** @type {string[]} */
    const out = [];
    block(main, out, "");
    return `${out.join("\n").replace(/\n{3,}/g, "\n\n").trim()}\n`;
  }

  /** @param {any} [env] */
  function read(env) {
    const doc = env?.doc ?? root.document;
    const model = ns.snapshot.domToModel(doc.body ?? doc.documentElement, env);
    const markdown = model ? toMarkdown(model) : "";
    const controls = countControls(model);
    return { markdown, url: String(root.location?.href ?? ""), title: String(doc.title ?? ""), controls, visibility: doc.visibilityState === "hidden" ? "hidden" : "visible" };
  }

  /** @param {any} node @returns {number} */
  function countControls(node) {
    if (!node || node.t !== "el" || node.hidden) return 0;
    return (ns.aria.isInteractive(ns.aria.roleOf(node)) ? 1 : 0) + (node.children ?? []).reduce((/** @type {number} */ n, /** @type {any} */ c) => n + countControls(c), 0);
  }

  ns.read = { toMarkdown, read, countControls };
})();
