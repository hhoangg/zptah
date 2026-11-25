// A native visual the desktop rendered inline reaches the web as a `visual` transcript item with its component tree.
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { Turn } from "@monoptah/protocol";
import { TranscriptActionsContext } from "./context";
import { readItem } from "./turnItems";
import { TranscriptTurn } from "./TranscriptTurn";

const show = (turn: Turn) =>
  render(
    <TranscriptActionsContext value={{}}>
      <TranscriptTurn turn={turn} />
    </TranscriptActionsContext>,
  );

const text = (value: string) => ({ kind: "text", text: value, size: "title", color: "ink" });

describe("visual transcript items", () => {
  it("are read with their title and tree", () => {
    expect(readItem({ type: "visual", id: "r4", title: "Deploys", tree: text("x") })).toEqual({ type: "visual", id: "r4", title: "Deploys", tree: text("x") });
    expect(readItem({ type: "visual", title: "no id" })).toBeNull();
  });

  it("draw the tree inline between the paragraphs of a reply", () => {
    show({
      id: "t",
      items: [
        { type: "text", id: "a", text: "Deploys per week" },
        {
          type: "visual",
          id: "v",
          title: "Deploys",
          tree: {
            kind: "column",
            gap: 8,
            children: [
              text("Weekly report"),
              { kind: "stat", label: "Deploys", value: "42", detail: "+12% vs last week", color: "accent", delta: null, trend: [1, 2, 3], lower_is_better: false },
              { kind: "chart", chart_type: "bar", series: [{ label: "d", values: [1, 3, 2] }], labels: ["a", "b", "c"], height: 160 },
              { kind: "tabs", labels: ["One", "Two"], selected: 1, handler: "1" },
              { kind: "canvas", width: 200, height: 100, shapes: [{ kind: "label", x: 1, y: 1, text: "on canvas", color: null, align: "start", baseline: "top", chip: false }] },
            ],
          },
        },
        { type: "text", id: "b", text: "That is the trend." },
      ],
    });
    const figure = screen.getByTestId("transcript-visual");
    expect(figure).toHaveAttribute("aria-label", "Deploys");
    expect(within(figure).getByText("Weekly report")).toBeInTheDocument();
    expect(within(figure).getByText("42")).toBeInTheDocument();
    expect(within(figure).getByText("on canvas")).toBeInTheDocument();
    expect(within(figure).getByText("Two")).toBeInTheDocument();
    expect(figure.querySelectorAll("svg").length).toBeGreaterThanOrEqual(3);
    const order = [...document.body.querySelectorAll("[data-testid='transcript-visual'], .md-p")].map((el) => el.textContent ?? "");
    expect(order[0]).toContain("Deploys per week");
    expect(order.at(-1)).toContain("That is the trend.");
  });

  it("draw a flow visual in the diagram viewer", async () => {
    show({
      id: "t",
      items: [
        {
          type: "visual",
          id: "v",
          title: "Approval flow",
          tree: { kind: "flow", title: "Approval flow", height: null, graph: { direction: "LR", nodes: [{ id: "a", label: "A", kind: "box", color: null }, { id: "b", label: "B", kind: "end", color: "positive" }], edges: [{ from: "a", to: "b", label: "go", dashed: false, arrow: true }] } },
        },
      ],
    });
    const viewer = await screen.findByTestId("visual-flow");
    expect(viewer.querySelectorAll("[data-flow-node]")).toHaveLength(2);
  });

  it("say so when the tree cannot be drawn, with the title as text", () => {
    const hostile = "<img src=x onerror=alert(1)>";
    const { container } = show({ id: "t", items: [{ type: "visual", id: "v", title: hostile, tree: { kind: "column" } }] });
    expect(screen.getByTestId("transcript-visual")).toHaveTextContent(`${hostile} · Open it on your desktop to see it.`);
    expect(container.querySelector("img")).toBeNull();
  });

  it("render hostile text nodes as text", () => {
    const hostile = '<script>alert(1)</script><img src=x onerror="alert(1)">';
    const { container } = show({ id: "t", items: [{ type: "visual", id: "v", title: hostile, tree: { kind: "column", gap: 4, children: [text(hostile), { kind: "stat", label: hostile, value: hostile, detail: hostile, color: "accent", delta: hostile, trend: [], lower_is_better: false }] } }] });
    expect(container.querySelector("script, img")).toBeNull();
    expect(container.textContent).toContain(hostile);
  });
});
