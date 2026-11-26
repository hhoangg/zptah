// A native visual the desktop rendered inline reaches the web as a `visual` transcript item with its component tree.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { Turn } from "@monoptah/protocol";
import { VisualController, type VisualReply, type VisualSend } from "@/features/visuals/VisualController";
import { TranscriptActionsContext, type TranscriptActions } from "./context";
import { readItem } from "./turnItems";
import { TranscriptTurn } from "./TranscriptTurn";

const show = (turn: Turn, actions: TranscriptActions = {}) =>
  render(
    <TranscriptActionsContext value={actions}>
      <TranscriptTurn turn={turn} />
    </TranscriptActionsContext>,
  );

const text = (value: string) => ({ kind: "text", text: value, size: "title", color: "ink" });

describe("visual transcript items", () => {
  it("are read with their title and tree", () => {
    expect(readItem({ type: "visual", id: "r4", title: "Deploys", tree: text("x") })).toEqual({ type: "visual", id: "r4", title: "Deploys", tree: text("x") });
    expect(readItem({ type: "visual", id: "r4", documentId: "d1", title: "Deploys", tree: text("x") })).toMatchObject({ documentId: "d1" });
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

describe("interactive visual items", () => {
  const tabsTree = (selected: number, handler: string) => ({
    kind: "column",
    gap: 4,
    children: [
      { kind: "tabs", labels: ["Bar", "Line", "Area"], selected, handler },
      { kind: "chart", chart_type: ["bar", "line", "area"][selected], series: [{ label: "d", values: [1, 3, 2] }], labels: ["a", "b", "c"], height: 120 },
    ],
  });
  const slider = (value: number, handler: string) => ({ kind: "slider", label: "Level", value, min: 0, max: 10, step: 1, handler });
  const turnOf = (tree: unknown): Turn => ({ id: "t1", items: [{ type: "visual", id: "r4", documentId: "d1", title: "Deploys", tree }] });
  const chart = () => new XMLSerializer().serializeToString(screen.getByTestId("transcript-visual").querySelector("svg")!);

  it("without the document id of the snapshot the controls stay disabled", () => {
    const visuals = new VisualController(vi.fn<VisualSend>());
    show({ id: "t1", items: [{ type: "visual", id: "r4", title: "Deploys", tree: tabsTree(0, "1:0") }] }, { visuals });
    for (const tab of screen.getAllByRole("tab")) expect(tab).toBeDisabled();
  });

  it("keeps the slot of a mounted visual when more than 64 are known", () => {
    const visuals = new VisualController(vi.fn<VisualSend>(async () => ({ tree: slider(2, "2:0"), reset: false })));
    const ref = { turnId: "t", itemId: "r1", documentId: "d1" };
    visuals.sync("mounted", ref, "snap", slider(1, "1:0"));
    const unsubscribe = visuals.subscribe("mounted", () => {});
    for (let i = 0; i < 80; i += 1) visuals.sync(`other-${i}`, ref, "snap", slider(1, "1:0"));
    expect(visuals.view("mounted")).toBeDefined();
    expect(visuals.view("other-0")).toBeUndefined();
    visuals.dispatch("mounted", "1:0", 2);
    expect(visuals.view("mounted")?.pending).toBe(true);
    unsubscribe();
  });

  it("without a controller the tabs are disabled and say to open the desktop", () => {
    show(turnOf(tabsTree(0, "1:0")));
    for (const tab of screen.getAllByRole("tab")) expect(tab).toBeDisabled();
    expect(screen.getAllByText("Open on desktop to interact")).toHaveLength(1);
  });

  it("a tapped tab goes to the desktop with its visual and handler, and the answer is drawn", async () => {
    const send = vi.fn<VisualSend>(async (_visual, _handler, value) => ({ tree: tabsTree(value as number, "2:0"), reset: false }));
    const visuals = new VisualController(send);
    show(turnOf(tabsTree(0, "1:0")), { visuals });
    expect(screen.queryByText("Open on desktop to interact")).toBeNull();
    const before = chart();
    await userEvent.click(screen.getByRole("tab", { name: "Area" }));
    expect(send).toHaveBeenCalledWith({ turnId: "t1", itemId: "r4", documentId: "d1" }, "1:0", 2);
    await waitFor(() => expect(screen.getByRole("tab", { name: "Area" })).toHaveAttribute("aria-selected", "true"));
    expect(chart()).not.toBe(before);
    // The next tap uses the handler of the tree it was drawn from.
    await userEvent.click(screen.getByRole("tab", { name: "Line" }));
    expect(send).toHaveBeenLastCalledWith({ turnId: "t1", itemId: "r4", documentId: "d1" }, "2:0", 1);
  });

  it("keeps its tree when the transcript redraws with the same snapshot and starts over when the snapshot changes", async () => {
    const visuals = new VisualController(async (_visual, _handler, value) => ({ tree: tabsTree(value as number, "2:0"), reset: false }));
    const turn = turnOf(tabsTree(0, "1:0"));
    const view = show(turn, { visuals });
    await userEvent.click(screen.getByRole("tab", { name: "Line" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Line" })).toHaveAttribute("aria-selected", "true"));
    // A watch update re-sends the item: the same snapshot, a new object.
    view.rerender(
      <TranscriptActionsContext value={{ visuals }}>
        <TranscriptTurn turn={structuredClone(turn)} />
      </TranscriptActionsContext>,
    );
    expect(screen.getByRole("tab", { name: "Line" })).toHaveAttribute("aria-selected", "true");
    // The desktop's snapshot changed: the controls' work is dropped.
    view.rerender(
      <TranscriptActionsContext value={{ visuals }}>
        <TranscriptTurn turn={turnOf(tabsTree(2, "1:0"))} />
      </TranscriptActionsContext>,
    );
    expect(screen.getByRole("tab", { name: "Area" })).toHaveAttribute("aria-selected", "true");
  });

  it("sends one event at a time with the handler of the latest tree and keeps only the newest of a control", async () => {
    const replies: ((reply: VisualReply) => void)[] = [];
    const send = vi.fn<VisualSend>(() => new Promise<VisualReply>((resolve) => replies.push(resolve)));
    const visuals = new VisualController(send);
    visuals.sync("k", { turnId: "t", itemId: "r1", documentId: "d1" }, "snap", slider(1, "1:0"));
    visuals.dispatch("k", "1:0", 2);
    visuals.dispatch("k", "1:0", 3);
    visuals.dispatch("k", "1:0", 4);
    expect(send).toHaveBeenCalledTimes(1);
    expect(visuals.view("k")?.pending).toBe(true);
    await act(async () => replies[0]!({ tree: slider(2, "2:0"), reset: false }));
    // Only the newest waiting value goes, with the new handler.
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenLastCalledWith({ turnId: "t", itemId: "r1", documentId: "d1" }, "2:0", 4);
    await act(async () => replies[1]!({ tree: slider(4, "3:0"), reset: false }));
    expect(visuals.view("k")).toMatchObject({ pending: false, error: null });
  });

  it("ignores a reply that arrives after the snapshot changed", async () => {
    const replies: ((reply: VisualReply) => void)[] = [];
    const visuals = new VisualController(() => new Promise<VisualReply>((resolve) => replies.push(resolve)));
    visuals.sync("k", { turnId: "t", itemId: "r1", documentId: "d1" }, "old", slider(1, "1:0"));
    visuals.dispatch("k", "1:0", 5);
    visuals.sync("k", { turnId: "t", itemId: "r1", documentId: "d1" }, "new", slider(9, "1:0"));
    await act(async () => replies[0]!({ tree: slider(5, "2:0"), reset: false }));
    const view = visuals.view("k");
    expect(view?.snapshot).toBe("new");
    expect(view?.node).toMatchObject({ kind: "slider", value: 9 });
    expect(view?.pending).toBe(false);
  });

  it("a stale reset replaces the tree and drops what waited, and a failure shows a short error", async () => {
    const replies: { ok: (reply: VisualReply) => void; fail: (error: Error) => void }[] = [];
    const send = vi.fn<VisualSend>(() => new Promise<VisualReply>((ok, fail) => replies.push({ ok, fail })));
    const visuals = new VisualController(send);
    visuals.sync("k", { turnId: "t", itemId: "r1", documentId: "d1" }, "snap", slider(1, "1:0"));
    visuals.dispatch("k", "1:0", 2);
    visuals.dispatch("k", "1:0", 3);
    await act(async () => replies[0]!.ok({ tree: slider(1, "1:0"), reset: true }));
    expect(send).toHaveBeenCalledTimes(1);
    expect(visuals.view("k")?.node).toMatchObject({ value: 1 });
    visuals.dispatch("k", "1:0", 6);
    await act(async () => replies[1]!.fail(Object.assign(new Error("busy"), { code: "busy" })));
    expect(visuals.view("k")?.error).toBe("Too many taps at once. Try again in a moment.");
    expect(visuals.view("k")?.pending).toBe(false);
  });
});

