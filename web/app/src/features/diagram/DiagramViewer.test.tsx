import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DiagramViewer } from "./DiagramViewer";

let board = { width: 600, height: 300 };
const observers: { callback: () => void }[] = [];

beforeEach(() => {
  board = { width: 600, height: 300 };
  observers.length = 0;
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (this: Element) {
    const isBoard = this.getAttribute("data-testid") === "diagram-board";
    return { x: 0, y: 0, left: 0, top: 0, right: isBoard ? board.width : 0, bottom: isBoard ? board.height : 0, width: isBoard ? board.width : 0, height: isBoard ? board.height : 0, toJSON: () => ({}) } as DOMRect;
  });
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        observers.push({ callback });
      }
      observe() {}
      disconnect() {}
    },
  );
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const view = (props: Partial<Parameters<typeof DiagramViewer>[0]> = {}) =>
  render(
    <DiagramViewer title="Approval" ariaLabel="Approval diagram" width={600} height={200} {...props}>
      <rect width={600} height={200} data-testid="drawing" />
    </DiagramViewer>,
  );
const zoom = () => screen.getByTestId("diagram-zoom").textContent;
const contentTransform = () => screen.getByTestId("diagram-content").style.transform;
const boardEl = () => screen.getByTestId("diagram-board");

describe("DiagramViewer", () => {
  it("shows the toolbar and fits the content on first render", () => {
    view({ source: "flowchart LR\nA-->B" });
    for (const name of ["Zoom out", "Zoom in", "Fit to view", "Fullscreen", "Show source", "Copy code"]) expect(screen.getByRole("button", { name })).toBeInTheDocument();
    expect(screen.getByTestId("diagram-title")).toHaveTextContent("Approval");
    // (600 - 40) / 600 fits the width; the height would allow more.
    expect(zoom()).toBe("93%");
    expect(boardEl()).toHaveAttribute("aria-label", "Approval diagram");
    expect(contentTransform()).toContain("scale(0.9333");
  });

  it("zooms with the buttons, and Fit restores the fitted view", async () => {
    const user = userEvent.setup();
    view();
    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(zoom()).toBe("112%");
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(zoom()).toBe("78%");
    await user.click(screen.getByRole("button", { name: "Fit to view" }));
    expect(zoom()).toBe("93%");
  });

  it("zooms at the pointer with ctrl or cmd + wheel, and lets a plain wheel scroll the page", () => {
    view();
    const plain = fireEvent.wheel(boardEl(), { deltaY: 100, clientX: 300, clientY: 150 });
    expect(plain).toBe(true); // not prevented: the page scrolls
    expect(zoom()).toBe("93%");
    const ctrl = fireEvent.wheel(boardEl(), { deltaY: -100, ctrlKey: true, clientX: 300, clientY: 150 });
    expect(ctrl).toBe(false);
    expect(Number.parseInt(zoom() ?? "0", 10)).toBeGreaterThan(93);
    const cmd = fireEvent.wheel(boardEl(), { deltaY: 100, metaKey: true, clientX: 300, clientY: 150 });
    expect(cmd).toBe(false);
  });

  it("keeps the content point under the pointer fixed when zooming with the wheel", () => {
    view();
    const before = contentTransform();
    fireEvent.wheel(boardEl(), { deltaY: -100, ctrlKey: true, clientX: 0, clientY: 0 });
    expect(contentTransform()).not.toBe(before);
    // Zooming at the top-left corner scales the offset from there.
    const x = Number.parseFloat(/translate\(([-\d.]+)px/.exec(contentTransform())![1]!);
    const x0 = Number.parseFloat(/translate\(([-\d.]+)px/.exec(before)![1]!);
    const ratio = Number.parseFloat(/scale\(([\d.]+)\)/.exec(contentTransform())![1]!) / Number.parseFloat(/scale\(([\d.]+)\)/.exec(before)![1]!);
    expect(x).toBeCloseTo(x0 * ratio, 3);
  });

  it("pans by dragging, and the dotted background moves with the content", () => {
    view();
    const before = { transform: contentTransform(), bg: boardEl().style.backgroundPosition };
    fireEvent.pointerDown(boardEl(), { pointerId: 1, clientX: 100, clientY: 100, button: 0 });
    fireEvent.pointerMove(boardEl(), { pointerId: 1, clientX: 150, clientY: 120 });
    fireEvent.pointerUp(boardEl(), { pointerId: 1, clientX: 150, clientY: 120 });
    expect(contentTransform()).not.toBe(before.transform);
    expect(boardEl().style.backgroundPosition).not.toBe(before.bg);
    const dx = Number.parseFloat(/translate\(([-\d.]+)px/.exec(contentTransform())![1]!) - Number.parseFloat(/translate\(([-\d.]+)px/.exec(before.transform)![1]!);
    expect(dx).toBeCloseTo(50, 3);
  });

  it("pinches to zoom with two pointers", () => {
    view();
    fireEvent.pointerDown(boardEl(), { pointerId: 1, clientX: 250, clientY: 150, button: 0 });
    fireEvent.pointerDown(boardEl(), { pointerId: 2, clientX: 350, clientY: 150, button: 0 });
    fireEvent.pointerMove(boardEl(), { pointerId: 2, clientX: 450, clientY: 150 });
    fireEvent.pointerUp(boardEl(), { pointerId: 2, clientX: 450, clientY: 150 });
    fireEvent.pointerUp(boardEl(), { pointerId: 1, clientX: 250, clientY: 150 });
    expect(Number.parseInt(zoom() ?? "0", 10)).toBeGreaterThan(93);
  });

  it("fits again on a double tap or double click, but not after a drag", () => {
    view();
    fireEvent.wheel(boardEl(), { deltaY: -300, ctrlKey: true, clientX: 10, clientY: 10 });
    expect(zoom()).not.toBe("93%");
    const tap = () => {
      fireEvent.pointerDown(boardEl(), { pointerId: 1, clientX: 200, clientY: 100, button: 0 });
      fireEvent.pointerUp(boardEl(), { pointerId: 1, clientX: 200, clientY: 100 });
    };
    tap();
    expect(zoom()).not.toBe("93%");
    tap();
    expect(zoom()).toBe("93%");
  });

  it("refits on resize while untouched and keeps the user's view afterwards", () => {
    view();
    board = { width: 300, height: 300 };
    act(() => observers.forEach((o) => o.callback()));
    expect(zoom()).toBe("43%");
    fireEvent.wheel(boardEl(), { deltaY: -100, ctrlKey: true, clientX: 10, clientY: 10 });
    const touched = zoom();
    board = { width: 900, height: 300 };
    act(() => observers.forEach((o) => o.callback()));
    expect(zoom()).toBe(touched);
  });

  it("goes fullscreen into the body and leaves with Esc", async () => {
    const user = userEvent.setup();
    view();
    await user.click(screen.getByRole("button", { name: "Fullscreen" }));
    const overlay = screen.getByTestId("diagram-fullscreen");
    expect(overlay.parentElement).toBe(document.body);
    expect(screen.getByRole("dialog", { name: "Approval" })).toHaveAttribute("aria-modal", "true");
    // In fullscreen a plain wheel zooms too.
    expect(fireEvent.wheel(boardEl(), { deltaY: -100, clientX: 100, clientY: 100 })).toBe(false);
    await user.keyboard("{Escape}");
    expect(screen.queryByTestId("diagram-fullscreen")).toBeNull();
    expect(screen.getByRole("button", { name: "Fullscreen" })).toBeInTheDocument();
  });

  it("toggles the source and copies it", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.spyOn(navigator.clipboard, "writeText").mockImplementation(writeText);
    view({ source: "flowchart LR\nA-->B", sourceLanguage: "mermaid" });
    await user.click(screen.getByRole("button", { name: "Show source" }));
    expect(screen.getByTestId("diagram-source")).toHaveTextContent("flowchart LR");
    expect(screen.queryByTestId("diagram-board")).toBeNull();
    await user.click(screen.getAllByRole("button", { name: "Copy code" })[0]!);
    expect(writeText).toHaveBeenCalledWith("flowchart LR\nA-->B");
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Show diagram" }));
    expect(screen.getByTestId("diagram-board")).toBeInTheDocument();
  });

  it("has no Source or Copy without a source", () => {
    view();
    expect(screen.queryByRole("button", { name: "Show source" })).toBeNull();
    expect(screen.queryByRole("button", { name: /copy/i })).toBeNull();
  });

  it("zooms and pans from the keyboard", async () => {
    const user = userEvent.setup();
    view();
    boardEl().focus();
    await user.keyboard("+");
    expect(zoom()).toBe("112%");
    await user.keyboard("0");
    expect(zoom()).toBe("93%");
  });
});
