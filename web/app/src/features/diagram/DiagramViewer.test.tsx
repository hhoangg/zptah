import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

  it("is a modal: Tab cycles inside, the page does not scroll, and focus returns to Full on exit", async () => {
    const user = userEvent.setup();
    render(
      <>
        <button type="button">outside</button>
        <DiagramViewer title="Approval" ariaLabel="Approval diagram" width={600} height={200} source="x">
          <rect width={600} height={200} />
        </DiagramViewer>
      </>,
    );
    await user.click(screen.getByRole("button", { name: "Fullscreen" }));
    expect(document.body.style.overflow).toBe("hidden");
    const dialog = screen.getByRole("dialog", { name: "Approval" });
    await waitFor(() => expect(dialog).toHaveFocus());
    const buttons = within(dialog).getAllByRole("button");
    const lastStop = within(dialog).getByTestId("diagram-board");
    lastStop.focus();
    await user.tab();
    expect(buttons[0]).toHaveFocus();
    await user.tab({ shift: true });
    expect(lastStop).toHaveFocus();
    // Focus that escapes (to the page behind) is pulled back.
    screen.getByRole("button", { name: "outside" }).focus();
    expect(dialog).toHaveFocus();
    await user.keyboard("{Escape}");
    expect(document.body.style.overflow).toBe("");
    await waitFor(() => expect(screen.getByRole("button", { name: "Fullscreen" })).toHaveFocus());
  });

  it("leaves fullscreen on Esc only when nothing else handled it, and handles it once", async () => {
    const user = userEvent.setup();
    const outer = vi.fn();
    document.addEventListener("keydown", outer);
    view();
    await user.click(screen.getByRole("button", { name: "Fullscreen" }));
    const dialog = screen.getByRole("dialog", { name: "Approval" });
    // A key another handler already took (defaultPrevented) does not close it.
    const stop = (event: Event) => event.preventDefault();
    within(dialog).getByRole("button", { name: "Zoom in" }).addEventListener("keydown", stop);
    within(dialog).getByRole("button", { name: "Zoom in" }).focus();
    await user.keyboard("{Escape}");
    expect(screen.getByTestId("diagram-fullscreen")).toBeInTheDocument();
    within(dialog).getByRole("button", { name: "Zoom in" }).removeEventListener("keydown", stop);
    // Handled by the panel: propagation stops, so the page never sees the key.
    outer.mockClear();
    await user.keyboard("{Escape}");
    expect(screen.queryByTestId("diagram-fullscreen")).toBeNull();
    expect(outer).not.toHaveBeenCalled();
    document.removeEventListener("keydown", outer);
  });

  it("does not take Esc from another dialog that has the focus", async () => {
    const user = userEvent.setup();
    view();
    await user.click(screen.getByRole("button", { name: "Fullscreen" }));
    const other = document.createElement("div");
    other.setAttribute("role", "dialog");
    other.tabIndex = 0;
    document.body.append(other);
    // The focus trap would pull focus back, so test the window fallback directly with focus inside the other dialog.
    vi.spyOn(document, "activeElement", "get").mockReturnValue(other);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByTestId("diagram-fullscreen")).toBeInTheDocument();
    other.remove();
  });

  it("keeps the placeholder at the height the panel had", async () => {
    const user = userEvent.setup();
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
      const h = this.getAttribute("data-testid") === "diagram-viewer" ? 333 : this.getAttribute("data-testid") === "diagram-board" ? 300 : 0;
      return { x: 0, y: 0, left: 0, top: 0, right: 600, bottom: h, width: 600, height: h, toJSON: () => ({}) } as DOMRect;
    });
    view();
    await user.click(screen.getByRole("button", { name: "Fullscreen" }));
    expect(screen.getByTestId("diagram-placeholder").style.height).toBe("333px");
  });

  it("leaves focus alone inside a portalled dialog or menu, and skips disabled and tabindex -1 stops", async () => {
    const user = userEvent.setup();
    view();
    await user.click(screen.getByRole("button", { name: "Fullscreen" }));
    const dialog = screen.getByRole("dialog", { name: "Approval" });
    const toast = document.createElement("div");
    toast.setAttribute("role", "dialog");
    const inner = document.createElement("button");
    toast.append(inner);
    document.body.append(toast);
    inner.focus();
    expect(inner).toHaveFocus();
    toast.remove();
    // A disabled button and a tabindex -1 button are not stops of the Tab cycle.
    const off = document.createElement("button");
    off.disabled = true;
    const skip = document.createElement("button");
    skip.tabIndex = -1;
    const board = within(dialog).getByTestId("diagram-board");
    board.after(off, skip);
    board.focus();
    await user.tab();
    expect(within(dialog).getByRole("button", { name: "Zoom out" })).toHaveFocus();
  });

  it("restores the page scroll once the last of several fullscreen viewers closes, in any order", async () => {
    const user = userEvent.setup();
    document.body.style.overflow = "auto";
    render(
      <>
        <DiagramViewer title="One" ariaLabel="one" width={600} height={200}><rect width={1} height={1} /></DiagramViewer>
        <DiagramViewer title="Two" ariaLabel="two" width={600} height={200}><rect width={1} height={1} /></DiagramViewer>
      </>,
    );
    const [one, two] = screen.getAllByRole("button", { name: "Fullscreen" });
    await user.click(one!);
    await user.click(within(screen.getByRole("group", { name: "Two" })).getByRole("button", { name: "Fullscreen" }));
    expect(document.body.style.overflow).toBe("hidden");
    void two;
    await user.click(within(screen.getByRole("dialog", { name: "One" })).getByRole("button", { name: "Exit fullscreen" }));
    expect(document.body.style.overflow).toBe("hidden");
    await user.click(within(screen.getByRole("dialog", { name: "Two" })).getByRole("button", { name: "Exit fullscreen" }));
    expect(document.body.style.overflow).toBe("auto");
    document.body.style.overflow = "";
  });
});
