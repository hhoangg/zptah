// ```mermaid blocks of a reply: every diagram is drawn by the `mermaid` library (mocked here at its boundary, because jsdom
// cannot lay out text) into a blob image inside the shared diagram viewer; a block that is still streaming, too large, or
// that does not parse stays source with a note. The real library under the production CSP is covered by
// web/e2e/tests/mermaid-csp.spec.ts.
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MermaidRender } from "@/features/diagram/mermaidRender";
import { useAppearance } from "@/lib/appearance";
import { Markdown } from "./Markdown";

const renderMermaid = vi.hoisted(() => vi.fn<(source: string, scheme: string, max: number) => Promise<MermaidRender>>());
vi.mock("@/features/diagram/mermaidRender", () => ({ renderMermaid }));

const FLOW = "```mermaid\nflowchart LR\n  A[Mono request] --> B{Risk?}\n  B -- safe --> C[Auto act]\n```";
const SEQUENCE = "```mermaid\nsequenceDiagram\n  Alice->>Bob: hi\n```";

const drawn = (width = 300, height = 120): MermaidRender => ({ ok: true, svg: `<svg width="${width}" height="${height}"></svg>`, width, height });
let created: string[] = [];
let revoked: string[] = [];
let imageFails = false;
let pendingImages: (() => void)[] | null = null;

beforeEach(() => {
  created = [];
  revoked = [];
  imageFails = false;
  pendingImages = null;
  renderMermaid.mockReset();
  renderMermaid.mockImplementation(async () => drawn());
  useAppearance.setState({ scheme: "dark" });
  vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: vi.fn(() => `blob:test/${created.push("u")}`), revokeObjectURL: vi.fn((url: string) => revoked.push(url)) }));
  // jsdom does not decode images: the block only needs the load or error event.
  vi.stubGlobal(
    "Image",
    class {
      onload: (() => void) | null = null;
      onerror: (() => void) | null = null;
      set src(_value: string) {
        const settle = () => (imageFails ? this.onerror : this.onload)?.();
        if (pendingImages) pendingImages.push(settle);
        else queueMicrotask(settle);
      }
    },
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("mermaid blocks", () => {
  it("draws a flowchart and a sequence diagram of one reply, each in its own viewer, with the source one click away", async () => {
    const user = userEvent.setup();
    renderMermaid.mockImplementation(async (source) => (source.startsWith("sequenceDiagram") ? drawn(400, 260) : drawn(300, 120)));
    const { container } = render(<Markdown text={`Here is the flow:\n\n${FLOW}\n\nAnd the sequence:\n\n${SEQUENCE}\n\nDone.`} />);
    const viewers = await screen.findAllByTestId("mermaid-diagram");
    expect(viewers).toHaveLength(2);
    expect(renderMermaid).toHaveBeenCalledTimes(2);
    expect(renderMermaid.mock.calls.map((call) => call[0])).toEqual(["flowchart LR\n  A[Mono request] --> B{Risk?}\n  B -- safe --> C[Auto act]", "sequenceDiagram\n  Alice->>Bob: hi"]);
    const hrefs = viewers.map((viewer) => within(viewer).getByTestId("mermaid-image").getAttribute("href"));
    expect(hrefs[0]).toMatch(/^blob:/);
    expect(hrefs[1]).toMatch(/^blob:/);
    expect(hrefs[0]).not.toBe(hrefs[1]);
    expect(within(viewers[1]!).getByTestId("diagram-content")).toHaveAttribute("viewBox", "0 0 400 260");
    for (const viewer of viewers) expect(within(viewer).getByTestId("diagram-title")).toHaveTextContent("Mermaid");
    expect(screen.queryByTestId("mermaid-fallback")).toBeNull();
    expect(screen.getByText("Done.")).toBeInTheDocument();
    // The raw block is not shown until Source is chosen, and what is drawn is an image, not markup.
    expect(container.querySelector(".md-code")).toBeNull();
    expect(container.querySelector("foreignObject, script, iframe, object")).toBeNull();
    await user.click(within(viewers[1]!).getByRole("button", { name: "Show source" }));
    expect(within(viewers[1]!).getByTestId("diagram-source")).toHaveTextContent("sequenceDiagram");
  });

  it("recognises the language tag in any case", async () => {
    render(<Markdown text={"```Mermaid\ngraph TD\nA-->B\n```"} />);
    expect(await screen.findByTestId("mermaid-diagram")).toBeInTheDocument();
  });

  it("draws with the app's colour scheme and again when it changes, releasing the old image", async () => {
    render(<Markdown text={FLOW} />);
    await screen.findByTestId("mermaid-diagram");
    expect(renderMermaid.mock.calls.at(-1)![1]).toBe("dark");
    act(() => useAppearance.setState({ scheme: "light" }));
    await vi.waitFor(() => expect(renderMermaid.mock.calls.at(-1)![1]).toBe("light"));
    await vi.waitFor(() => expect(revoked).toHaveLength(1));
    expect(await screen.findByTestId("mermaid-diagram")).toBeInTheDocument();
  });

  it("releases the image when the block goes away", async () => {
    const { unmount } = render(<Markdown text={FLOW} />);
    await screen.findByTestId("mermaid-diagram");
    expect(revoked).toHaveLength(0);
    unmount();
    expect(revoked).toHaveLength(1);
  });

  it("releases the image of a block that went away while the browser was decoding it", async () => {
    pendingImages = [];
    const { unmount } = render(<Markdown text={FLOW} />);
    await vi.waitFor(() => expect(pendingImages).toHaveLength(1));
    unmount();
    expect(revoked).toHaveLength(0);
    act(() => pendingImages!.forEach((settle) => settle()));
    await vi.waitFor(() => expect(revoked).toHaveLength(1));
  });

  it("shows a diagram that does not parse as source with a short note", async () => {
    renderMermaid.mockResolvedValue({ ok: false, error: "Parse error on line 2" });
    render(<Markdown text={"```mermaid\nflowchart LR\n  A --> --> B\n```"} />);
    const fallback = await screen.findByTestId("mermaid-fallback");
    expect(fallback).toHaveTextContent("flowchart LR");
    expect(fallback).toHaveTextContent("Shown as source: could not draw this diagram (Parse error on line 2).");
    expect(screen.queryByTestId("mermaid-diagram")).toBeNull();
  });

  it("keeps drawing the other blocks when one of them fails", async () => {
    renderMermaid.mockImplementation(async (source) => (source.startsWith("flowchart") ? { ok: false, error: "boom" } : drawn()));
    render(<Markdown text={`${FLOW}\n\n${SEQUENCE}`} />);
    expect(await screen.findByTestId("mermaid-fallback")).toHaveTextContent("boom");
    expect(await screen.findAllByTestId("mermaid-diagram")).toHaveLength(1);
  });

  it("falls back to source when the browser refuses the drawing", async () => {
    imageFails = true;
    render(<Markdown text={FLOW} />);
    expect(await screen.findByTestId("mermaid-fallback")).toHaveTextContent(/could not draw this diagram \(the browser could not show the result\)/);
    expect(revoked).toHaveLength(1);
  });

  it("keeps a block that is still streaming as source until its fence closes", async () => {
    const open = "```mermaid\nflowchart LR\n  A --> B";
    const { rerender } = render(<Markdown text={open} streaming />);
    expect(screen.queryByTestId("mermaid-diagram")).toBeNull();
    expect(screen.getByText(/flowchart LR/)).toBeInTheDocument();
    expect(renderMermaid).not.toHaveBeenCalled();
    rerender(<Markdown text={`${open}\n\`\`\``} streaming={false} />);
    expect(await screen.findByTestId("mermaid-diagram")).toBeInTheDocument();
    expect(renderMermaid).toHaveBeenCalledTimes(1);
  });

  it("shows an oversized block as source without drawing it", () => {
    const big = `\`\`\`mermaid\nflowchart LR\n${"A --> B\n".repeat(600)}\`\`\``;
    render(<Markdown text={big} />);
    expect(screen.getByTestId("mermaid-fallback")).toHaveTextContent(/too large/);
    const wide = `\`\`\`mermaid\nflowchart LR\nA[${"x".repeat(33 * 1024)}] --> B\n\`\`\``;
    const { unmount } = render(<Markdown text={wide} />);
    expect(screen.getAllByTestId("mermaid-fallback")).toHaveLength(2);
    expect(renderMermaid).not.toHaveBeenCalled();
    unmount();
  });

  it("shows hostile source as text, and never as an element", async () => {
    renderMermaid.mockResolvedValue({ ok: false, error: "no" });
    const source = '```mermaid\nflowchart LR\n  A["<script>alert(1)</script>"] -->|"<img src=x onerror=alert(1)>"| B["<svg onload=alert(1)>"]\n```';
    const { container } = render(<Markdown text={source} />);
    const fallback = await screen.findByTestId("mermaid-fallback");
    expect(fallback).toHaveTextContent("<script>alert(1)</script>");
    expect(container.querySelector("script, img, iframe, object, foreignObject, style")).toBeNull();
    for (const el of container.querySelectorAll("*")) expect([...el.attributes].some((a) => /^on/i.test(a.name))).toBe(false);
  });
});
