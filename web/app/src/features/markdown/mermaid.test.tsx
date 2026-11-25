// ```mermaid blocks of a reply: the flowchart subset is drawn in the shared diagram viewer, anything else (and a block
// that is still streaming) stays source. mermaid.js is never loaded; every label is text, so hostile ones stay text.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { Markdown } from "./Markdown";

const FLOW = "```mermaid\nflowchart LR\n  A[Mono request] --> B{Risk?}\n  B -- safe --> C[Auto act]\n  B -- risky --> D[Ask user]\n```";

describe("mermaid blocks", () => {
  it("draws a flowchart in the viewer with its source one click away", async () => {
    const user = userEvent.setup();
    const { container } = render(<Markdown text={`Here is the flow:\n\n${FLOW}\n\nDone.`} />);
    const viewer = await screen.findByTestId("mermaid-diagram");
    expect(within(viewer).getByTestId("diagram-title")).toHaveTextContent("Mermaid");
    expect(viewer.querySelectorAll("[data-flow-node]")).toHaveLength(4);
    expect(within(viewer).getByText("Mono request")).toBeInTheDocument();
    expect(within(viewer).getByText("safe")).toBeInTheDocument();
    expect(screen.getByText("Done.")).toBeInTheDocument();
    // The raw block is not shown until Source is chosen.
    expect(container.querySelector(".md-code")).toBeNull();
    await user.click(within(viewer).getByRole("button", { name: "Show source" }));
    expect(within(viewer).getByTestId("diagram-source")).toHaveTextContent("flowchart LR");
    expect(within(viewer).getByTestId("diagram-source")).toHaveTextContent("B -- safe --> C[Auto act]");
  });

  it("recognises the language tag in any case", async () => {
    render(<Markdown text={"```Mermaid\ngraph TD\nA-->B\n```"} />);
    expect(await screen.findByTestId("mermaid-diagram")).toBeInTheDocument();
  });

  it("shows other diagram types as source, with a note, and never loads mermaid.js", () => {
    render(<Markdown text={"```mermaid\nsequenceDiagram\n  A->>B: hi\n```"} />);
    const fallback = screen.getByTestId("mermaid-fallback");
    expect(fallback).toHaveTextContent("sequenceDiagram");
    expect(fallback).toHaveTextContent(/Shown as source/);
    expect(screen.queryByTestId("mermaid-diagram")).toBeNull();
  });

  it("shows a flowchart with unsupported syntax as source", async () => {
    render(<Markdown text={"```mermaid\nflowchart LR\nsubgraph s\nA-->B\nend\n```"} />);
    await waitFor(() => expect(screen.getByTestId("mermaid-fallback")).toBeInTheDocument());
  });

  it("keeps a block that is still streaming as source until its fence closes", async () => {
    const open = "```mermaid\nflowchart LR\n  A --> B";
    const { rerender } = render(<Markdown text={open} streaming />);
    expect(screen.queryByTestId("mermaid-diagram")).toBeNull();
    expect(screen.getByText(/flowchart LR/)).toBeInTheDocument();
    rerender(<Markdown text={`${open}\n\`\`\``} streaming={false} />);
    expect(await screen.findByTestId("mermaid-diagram")).toBeInTheDocument();
  });

  it("renders hostile labels as text and creates no script or image element", async () => {
    const warn = vi.spyOn(console, "error").mockImplementation(() => {});
    const source = '```mermaid\nflowchart LR\n  A["<script>alert(1)</script>"] -->|"<img src=x onerror=alert(1)>"| B["<svg onload=alert(1)>"]\n```';
    const { container } = render(<Markdown text={source} />);
    const viewer = await screen.findByTestId("mermaid-diagram");
    expect(container.querySelector("script, img, iframe, object, foreignObject, style")).toBeNull();
    expect(viewer.querySelector("[data-flow-node='A'] text")!.textContent).toBe("<script>alert(1)</script>");
    expect(within(viewer).getByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(within(viewer).getByText("<svg onload=alert(1)>")).toBeInTheDocument();
    // Nothing carries an event handler attribute.
    for (const el of container.querySelectorAll("*")) expect([...el.attributes].some((a) => /^on/i.test(a.name))).toBe(false);
    warn.mockRestore();
  });

  it("shows an oversized block as source without parsing it", () => {
    const big = `\`\`\`mermaid\nflowchart LR\n${"A --> B\n".repeat(600)}\`\`\``;
    render(<Markdown text={big} />);
    expect(screen.getByTestId("mermaid-fallback")).toHaveTextContent(/too large/);
    const wide = `\`\`\`mermaid\nflowchart LR\nA[${"x".repeat(33 * 1024)}] --> B\n\`\`\``;
    const { unmount } = render(<Markdown text={wide} />);
    expect(screen.getAllByTestId("mermaid-fallback")).toHaveLength(2);
    unmount();
  });
});
