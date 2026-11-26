// The Mermaid renderer around the (mocked) `mermaid` library: one configuration, unique ids, one render at a time, a short
// message on an error, nothing left in the document, the diverted theme CSS written into the image, and the SVG sizing.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installTrustedTypesPolicy } from "@/trusted-types";
import { RENDER_TIMEOUT_MS, renderMermaid, sanitizeFragment, sizeSvg } from "./mermaidRender";

type Options = NonNullable<Parameters<TrustedTypePolicyFactory["createPolicy"]>[1]>;

const api = vi.hoisted(() => ({
  initialize: vi.fn(),
  parse: vi.fn(),
  render: vi.fn(),
}));
vi.mock("mermaid", () => ({ default: api }));
const sanitize = vi.hoisted(() => vi.fn((html: string, _config?: unknown) => `clean(${html})`));
vi.mock("dompurify", () => ({ default: { sanitize } }));

const SVG = (id: string, extra = "") => `<svg id="${id}" width="100%" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200.4 100.2" style="max-width: 200px;">${extra}</svg>`;

let policy: Options = {};
beforeEach(() => {
  api.initialize.mockReset();
  api.parse.mockReset().mockResolvedValue({ diagramType: "flowchart" });
  api.render.mockReset().mockImplementation(async (id: string) => ({ svg: SVG(id) }));
  window.trustedTypes = {
    createPolicy: (_name: string, options?: Options) => {
      policy = options ?? {};
      return {};
    },
  };
  installTrustedTypesPolicy();
});
afterEach(() => {
  delete window.trustedTypes;
  document.body.replaceChildren();
});

describe("renderMermaid", () => {
  it("configures mermaid for strict security, SVG labels and the app scheme", async () => {
    await renderMermaid("flowchart LR\nA-->B", "dark", 1000);
    await renderMermaid("flowchart LR\nA-->B", "light", 1000);
    const [dark, light] = api.initialize.mock.calls.map((call) => call[0]);
    expect(dark).toMatchObject({ startOnLoad: false, securityLevel: "strict", theme: "dark", htmlLabels: false, suppressErrorRendering: true, maxTextSize: 1000 });
    expect(dark.flowchart).toMatchObject({ htmlLabels: false });
    expect(light.theme).toBe("default");
    // A diagram directive may not undo any of that.
    for (const key of ["securityLevel", "themeCSS", "theme", "startOnLoad", "htmlLabels"]) expect(dark.secure).toContain(key);
    expect(api.parse).toHaveBeenCalledWith("flowchart LR\nA-->B", { suppressErrors: false });
  });

  it("draws with a unique id each time, including calls made at the same moment", async () => {
    const results = await Promise.all([renderMermaid("a", "dark", 10), renderMermaid("b", "dark", 10), renderMermaid("c", "dark", 10)]);
    const ids = api.render.mock.calls.map((call) => call[0] as string);
    expect(new Set(ids).size).toBe(3);
    for (const id of ids) expect(id).toMatch(/^zptah-mermaid-\d+$/);
    expect(results.every((result) => result.ok)).toBe(true);
  });

  it("runs one render at a time, in order", async () => {
    let running = 0;
    let peak = 0;
    const order: string[] = [];
    api.render.mockImplementation(async (id: string, source: string) => {
      running++;
      peak = Math.max(peak, running);
      order.push(source);
      await new Promise((resolve) => setTimeout(resolve, 5));
      running--;
      return { svg: SVG(id) };
    });
    await Promise.all(["one", "two", "three"].map((source) => renderMermaid(source, "dark", 10)));
    expect(peak).toBe(1);
    expect(order).toEqual(["one", "two", "three"]);
  });

  it("returns a one-line message for a parse error and does not draw", async () => {
    api.parse.mockRejectedValue(new Error("Parse error on line 2:\n...A --> --> B\n----^"));
    const result = await renderMermaid("flowchart LR\nA --> --> B", "dark", 100);
    expect(result).toEqual({ ok: false, error: "Parse error on line 2:" });
    expect(api.render).not.toHaveBeenCalled();
  });

  it("removes what mermaid leaves in the document after a failed render, and keeps going", async () => {
    api.render.mockImplementationOnce(async (id: string) => {
      for (const leftover of [`d${id}`, id]) {
        const element = document.createElement("div");
        element.id = leftover;
        document.body.append(element);
      }
      throw new Error("layout failed");
    });
    expect(await renderMermaid("a", "dark", 10)).toEqual({ ok: false, error: "layout failed" });
    expect(document.body.children).toHaveLength(0);
    expect((await renderMermaid("b", "dark", 10)).ok).toBe(true);
  });

  it("sizes the image from the viewBox and drops the responsive attributes", async () => {
    const result = await renderMermaid("a", "dark", 10);
    expect(result).toMatchObject({ ok: true, width: 201, height: 101 });
    if (!result.ok) return;
    const root = /<svg\b[^>]*>/.exec(result.svg)![0];
    expect(root).toContain('width="201" height="101"');
    expect(root).not.toContain("max-width");
    expect(root).not.toContain('width="100%"');
    expect(root).toContain('xmlns:xlink="http://www.w3.org/1999/xlink"');
  });

  it("puts the theme CSS the policy diverted into the image as escaped text", async () => {
    api.render.mockImplementation(async (id: string) => {
      // What mermaid does: `style.innerHTML = rules`, which goes through the default policy and writes "" into the page.
      expect(policy.createHTML?.(`#${id}{fill:red;}#${id} g>rect{x:1}`)).toBe("");
      return { svg: SVG(id, "<style></style>") };
    });
    const result = await renderMermaid("a", "dark", 10);
    expect(result.ok && result.svg).toContain("<style>#zptah-mermaid-");
    expect(result.ok && result.svg).toContain("g&gt;rect{x:1}</style>");
    // The diversion ends with the render: the policy refuses everything again.
    expect(() => policy.createHTML?.("#zptah-mermaid-1{x}")).toThrow("HTML sinks are disabled");
    expect(() => policy.createHTML?.("<b>x</b>")).toThrow("HTML sinks are disabled");
  });

  it("sanitizes the other HTML mermaid writes with a strict DOMPurify config", async () => {
    api.render.mockImplementation(async (id: string) => {
      expect(policy.createHTML?.("<i>icon</i>")).toBe("clean(<i>icon</i>)");
      return { svg: SVG(id) };
    });
    await renderMermaid("a", "dark", 10);
    const config = sanitize.mock.calls[0]![1] as { FORBID_TAGS: string[]; USE_PROFILES: object };
    expect(config.FORBID_TAGS).toEqual(expect.arrayContaining(["script", "style", "iframe", "foreignObject"]));
    expect(config.USE_PROFILES).toEqual({ html: true, svg: true, svgFilters: true });
  });

  it("writes a non-breaking space as a numeric entity, which XML accepts", async () => {
    api.render.mockImplementation(async (id: string) => ({ svg: SVG(id, "<text>a&nbsp;b &amp; c</text>") }));
    const result = await renderMermaid("a", "dark", 10);
    expect(result.ok && result.svg).toContain("a&#160;b &amp; c");
    expect(result.ok && result.svg).not.toContain("&nbsp;");
  });

  it("gives up on a render that hangs, moves on to the next, and keeps the next one's capture clean", async () => {
    vi.useFakeTimers();
    try {
      let stuckId = "";
      api.render.mockImplementationOnce((id: string) => {
        stuckId = id;
        const scratch = document.createElement("div");
        scratch.id = `d${id}`;
        document.body.append(scratch);
        return new Promise(() => {});
      });
      const first = renderMermaid("stuck", "dark", 10);
      const second = renderMermaid("fine", "dark", 10);
      await vi.advanceTimersByTimeAsync(RENDER_TIMEOUT_MS + 10);
      expect(await first).toEqual({ ok: false, error: "timed out" });
      expect((await second).ok).toBe(true);
      expect(stuckId).toMatch(/^zptah-mermaid-/);
      expect(document.getElementById(`d${stuckId}`)).toBeNull();
      // The capture of the stuck render ended with the timeout: nothing is sanitized and let through any more.
      expect(() => policy.createHTML?.("<b>x</b>")).toThrow("HTML sinks are disabled");
      expect(() => policy.createHTML?.(`#${stuckId}{late:1}`)).toThrow("HTML sinks are disabled");
    } finally {
      vi.useRealTimers();
    }
  });

  it("never takes a capture for a render whose timeout fired during a slow parse", async () => {
    vi.useFakeTimers();
    try {
      let finishParse: () => void = () => {};
      api.parse.mockImplementationOnce(() => new Promise((resolve) => { finishParse = () => resolve({ diagramType: "flowchart" }); }));
      const slow = renderMermaid("slow", "dark", 10);
      await vi.advanceTimersByTimeAsync(RENDER_TIMEOUT_MS + 10);
      expect(await slow).toEqual({ ok: false, error: "timed out" });
      // The abandoned render now gets its parse back; it must stop there, without drawing or holding a capture.
      finishParse();
      await vi.advanceTimersByTimeAsync(10);
      expect(api.render).not.toHaveBeenCalled();
      expect(() => policy.createHTML?.("<b>x</b>")).toThrow("HTML sinks are disabled");
      // And the queue is not blocked or disturbed by it.
      expect((await renderMermaid("next", "dark", 10)).ok).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it("fails when the result has no size", async () => {
    api.render.mockResolvedValue({ svg: "<svg></svg>" });
    expect(await renderMermaid("a", "dark", 10)).toEqual({ ok: false, error: "the diagram has no size" });
  });
});

describe("sizeSvg", () => {
  it("rejects a missing or absurd viewBox", () => {
    expect(sizeSvg("<svg></svg>")).toBeNull();
    expect(sizeSvg('<svg viewBox="0 0 0 0"></svg>')).toBeNull();
    expect(sizeSvg('<svg viewBox="0 0 90000 10"></svg>')).toBeNull();
    expect(sizeSvg("no svg")).toBeNull();
  });
  it("keeps an existing xlink namespace", () => {
    const sized = sizeSvg('<svg xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10"></svg>');
    expect(sized?.svg.match(/xmlns:xlink/g)).toHaveLength(1);
  });
});

describe("sanitizeFragment", () => {
  it("sanitizes SVG markup inside an svg wrapper and removes the wrapper, and HTML as it is", () => {
    const purify = { sanitize: vi.fn((html: string, _config: object) => html) };
    expect(sanitizeFragment(purify, '<g><svg width="1"></svg></g>')).toBe('<g><svg width="1"></svg></g>');
    expect(purify.sanitize.mock.calls[0]![0]).toBe('<svg><g><svg width="1"></svg></g></svg>');
    expect(sanitizeFragment(purify, "<span>label</span>")).toBe("<span>label</span>");
    expect(purify.sanitize.mock.calls[1]![0]).toBe("<span>label</span>");
  });

  it("returns nothing when the sanitized SVG lost its wrapper", () => {
    expect(sanitizeFragment({ sanitize: () => "" }, "<g></g>")).toBe("");
  });
});
