// Content script entry. Injected on demand by the worker through chrome.scripting into the
// isolated world; it only answers `run(op, args)` calls and never touches the page's scripts.
// The build wraps all files of this directory into one IIFE, so these are plain scripts.
(() => {
  const root = /** @type {any} */ (globalThis);
  const ns = (root.zptahContent ??= {});
  ns.version = 1;

  /** @param {string} s */
  const fold = (s) => ns.find.fold(s);

  /**
   * @param {string} op
   * @param {any} args
   */
  function run(op, args) {
    try {
      switch (op) {
        case "snapshot":
          return ns.snapshot.snapshot(args);
        case "read":
          return ns.read.read();
        case "find":
          return ns.find.find(args);
        case "state": {
          const doc = root.document;
          const text = String(doc.body?.innerText ?? "");
          const hay = fold(text);
          return {
            visible: doc.visibilityState === "visible",
            url: String(root.location?.href ?? ""),
            title: String(doc.title ?? ""),
            has: typeof args?.text === "string" ? hay.includes(fold(args.text)) : null,
            gone: typeof args?.text_gone === "string" ? !hay.includes(fold(args.text_gone)) : null,
            ready: doc.readyState === "complete",
          };
        }
        case "rect": {
          if (ns.snapshot.state.generation !== args.generation) return { stale: true };
          const el = ns.snapshot.resolveRef(args.ref);
          if (!el) return { stale: true };
          const r = el.getBoundingClientRect();
          return {
            x: r.x, y: r.y, width: r.width, height: r.height,
            viewport: { width: root.innerWidth, height: root.innerHeight },
            dpr: root.devicePixelRatio || 1,
          };
        }
        default:
          return { error: `unknown op ${op}` };
      }
    } catch (err) {
      return { error: String(err instanceof Error ? err.message : err).slice(0, 200) };
    }
  }

  ns.run = run;
  ns.zptahContentLoaded = true;
})();
