import { afterEach, describe, expect, it, vi } from "vitest";
import { captureMermaidStyle, installTrustedTypesPolicy } from "./trusted-types";

type Options = NonNullable<Parameters<TrustedTypePolicyFactory["createPolicy"]>[1]>;

function installAndCapture(): { name: string; options: Options } {
  let captured: { name: string; options: Options } | undefined;
  window.trustedTypes = {
    createPolicy: (name: string, options?: Options) => {
      captured = { name, options: options ?? {} };
      return {};
    },
  };
  installTrustedTypesPolicy();
  if (!captured) throw new Error("no policy created");
  return captured;
}

afterEach(() => {
  delete window.trustedTypes;
  vi.restoreAllMocks();
});

describe("Trusted Types default policy", () => {
  it("is named default", () => {
    expect(installAndCapture().name).toBe("default");
  });

  it("blocks HTML and script sinks", () => {
    const { options } = installAndCapture();
    expect(() => options.createHTML?.("<img src=x onerror=alert(1)>")).toThrow("HTML sinks are disabled");
    expect(() => options.createScript?.("alert(1)")).toThrow("Script sinks are disabled");
  });

  it("allows /sw.js and same-origin /assets/*.js", () => {
    const { options } = installAndCapture();
    const origin = location.origin;
    expect(options.createScriptURL?.("/sw.js")).toBe(`${origin}/sw.js`);
    expect(options.createScriptURL?.("/assets/index-abc123.js")).toBe(`${origin}/assets/index-abc123.js`);
    expect(options.createScriptURL?.(`${origin}/assets/chunk.js`)).toBe(`${origin}/assets/chunk.js`);
  });

  it("blocks foreign origins, other paths and non-js assets", () => {
    const { options } = installAndCapture();
    expect(() => options.createScriptURL?.("https://evil.example/assets/x.js")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("https://evil.example/sw.js")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("/other.js")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("/assets/data.json")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("javascript:alert(1)")).toThrow("Blocked script URL");
    expect(() => options.createScriptURL?.("data:text/javascript,alert(1)")).toThrow("Blocked script URL");
  });

  it("does nothing where Trusted Types do not exist", () => {
    delete window.trustedTypes;
    expect(() => installTrustedTypesPolicy()).not.toThrow();
  });
});

describe("Mermaid capture window", () => {
  const sanitize = (html: string) => `clean(${html})`;

  it("empties the render's own theme CSS and sanitizes other writes, only while it holds the capture", async () => {
    const { options } = installAndCapture();
    expect(() => options.createHTML?.("<b>x</b>")).toThrow("HTML sinks are disabled");
    const { value, rules } = await captureMermaidStyle(
      "zptah-mermaid-7",
      async () => {
        expect(options.createHTML?.("#zptah-mermaid-7{fill:red;}")).toBe("");
        // Anything else is sanitized by the renderer's function, never passed through raw.
        expect(options.createHTML?.("<img src=x onerror=alert(1)>")).toBe("clean(<img src=x onerror=alert(1)>)");
        expect(options.createHTML?.("#zptah-mermaid-7 x")).toBe("clean(#zptah-mermaid-7 x)");
        expect(options.createHTML?.("#zptah-mermaid-8{fill:red;}")).toBe("clean(#zptah-mermaid-8{fill:red;})");
        // Script and script URL sinks stay refused inside the window.
        expect(() => options.createScript?.("alert(1)")).toThrow("Script sinks are disabled");
        expect(() => options.createScriptURL?.("https://evil.example/x.js")).toThrow("Blocked script URL");
        return 42;
      },
      sanitize,
    );
    expect(value).toBe(42);
    expect(rules).toBe("#zptah-mermaid-7{fill:red;}");
    expect(() => options.createHTML?.("#zptah-mermaid-7{fill:red;}")).toThrow("HTML sinks are disabled");
    expect(() => options.createHTML?.("<b>x</b>")).toThrow("HTML sinks are disabled");
  });

  it("is released when the render throws", async () => {
    const { options } = installAndCapture();
    await expect(captureMermaidStyle("a", async () => { throw new Error("x"); }, sanitize)).rejects.toThrow("x");
    expect(() => options.createHTML?.("#a{}")).toThrow("HTML sinks are disabled");
  });

  it("keeps captures apart by id: a slow render's late CSS goes to itself, not the next one", async () => {
    const { options } = installAndCapture();
    let finishSlow: () => void = () => {};
    const slow = captureMermaidStyle("slow", () => new Promise<void>((resolve) => { finishSlow = resolve; }), sanitize);
    const fast = await captureMermaidStyle("fast", async () => {
      options.createHTML?.("#slow{late:1}");
      options.createHTML?.("#fast{mine:1}");
    }, sanitize);
    expect(fast.rules).toBe("#fast{mine:1}");
    finishSlow();
    expect((await slow).rules).toBe("#slow{late:1}");
  });
});
