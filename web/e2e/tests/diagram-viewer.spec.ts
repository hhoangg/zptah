// The diagram viewer and the native visuals in a transcript, in headless Chromium: a Mermaid flowchart, a Mermaid
// diagram the web app shows as source, a `visual.flow`, a canvas with anchored labels, and a stat with a chart, against
// the dev preview of the app (an in-browser mock desktop whose `sess-visuals` session carries them), which the spec
// serves itself with `vite`. It needs no relay, account or passkey. Two environments: a mouse desktop and a 390 px phone
// with real touch input through CDP (pinch, double tap, drag).
//
// Set DIAGRAM_SHOTS=<dir> to also save screenshots there.
import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromium, expect, test, type Browser, type BrowserContext, type Locator, type Page } from "@playwright/test";
import { webDir } from "../lib/paths.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const SESSION = (theme: string) => `/src/features/sessions/dev/?to=/d/${DESKTOP_ID}/s/sess-visuals&theme=${theme}`;
const SHOTS = process.env["DIAGRAM_SHOTS"];

async function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      const port = typeof address === "object" && address !== null ? address.port : 0;
      server.close(() => resolve(port));
    });
  });
}

const zoomOf = async (viewer: Locator): Promise<number> => Number.parseInt((await viewer.getByTestId("diagram-zoom").textContent()) ?? "0", 10);
const transformOf = (viewer: Locator): Promise<string> => viewer.getByTestId("diagram-content").evaluate((el) => (el as SVGElement).style.transform);

/** Two fingers (pinch) and one finger (drag, taps) through CDP, so the browser's own touch handling is in play. */
async function touch(page: Page, steps: { type: "touchStart" | "touchMove" | "touchEnd"; points: { x: number; y: number; id: number }[]; wait?: number }[]): Promise<void> {
  const cdp = await page.context().newCDPSession(page);
  for (const step of steps) {
    if (step.wait) await page.waitForTimeout(step.wait);
    await cdp.send("Input.dispatchTouchEvent", { type: step.type, touchPoints: step.points });
  }
  await cdp.detach();
}

test.describe("diagram viewer and native visuals", () => {
  test.describe.configure({ mode: "serial" });
  let vite: ChildProcess | null = null;
  let browser: Browser;
  let context: BrowserContext;
  let page: Page;
  let base = "";
  let stateDir = "";

  test.beforeAll(async () => {
    test.setTimeout(120_000);
    const port = await freePort();
    base = `http://localhost:${port}`;
    stateDir = mkdtempSync(join(tmpdir(), "monoptah-e2e-diagrams-"));
    vite = spawn(join(webDir, "app", "node_modules", ".bin", "vite"), ["--port", String(port), "--strictPort", "--host", "localhost"], {
      cwd: join(webDir, "app"),
      env: { ...process.env, VITE_MOCK_API: "1", MONOPTAH_PREVIEW_STATE: stateDir },
      stdio: "ignore",
    });
    await expect.poll(async () => (await fetch(`${base}${SESSION("dark")}`).catch(() => null))?.status ?? 0, { timeout: 90_000, intervals: [500] }).toBe(200);
    browser = await chromium.launch({ headless: true });
    if (SHOTS) mkdirSync(SHOTS, { recursive: true });
  });

  test.afterAll(async () => {
    await browser?.close();
    vite?.kill("SIGTERM");
    if (stateDir) rmSync(stateDir, { recursive: true, force: true });
  });

  test.afterEach(async () => {
    await context?.close();
  });

  async function open(options: Parameters<Browser["newContext"]>[0], theme = "dark"): Promise<void> {
    context = await browser.newContext(options);
    await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: base }).catch(() => {});
    page = await context.newPage();
    await page.goto(`${base}${SESSION(theme)}`);
    await expect(page.getByTestId("mermaid-diagram")).toBeVisible({ timeout: 60_000 });
    await expect(page.getByTestId("visual-flow")).toBeVisible();
  }
  async function shot(name: string): Promise<void> {
    if (SHOTS) await page.screenshot({ path: join(SHOTS, `${name}.png`) });
  }

  test("desktop: mermaid is drawn, panned, zoomed at the pointer, fitted, fullscreened and copied", async () => {
    await open({ viewport: { width: 1100, height: 900 } });
    const viewer = page.getByTestId("mermaid-diagram");
    await viewer.scrollIntoViewIfNeeded();
    await expect(viewer.locator("[data-flow-node]")).toHaveCount(6);
    await expect(viewer.getByText("Mono request")).toBeVisible();
    await expect(viewer.getByText("risky", { exact: true })).toBeVisible();
    const fitted = await zoomOf(viewer);
    expect(fitted).toBeGreaterThan(30);
    // The background is dotted and keeps pace with the content.
    const board = viewer.getByTestId("diagram-board");
    expect(await board.evaluate((el) => getComputedStyle(el).backgroundImage)).toContain("radial-gradient");
    await shot("desktop-mermaid-dark");

    // Drag pans the content and the dots together.
    const box = (await board.boundingBox())!;
    const before = { transform: await transformOf(viewer), dots: await board.evaluate((el) => el.style.backgroundPosition) };
    await page.mouse.move(box.x + 100, box.y + 100);
    await page.mouse.down();
    await page.mouse.move(box.x + 160, box.y + 130, { steps: 5 });
    await page.mouse.up();
    expect(await transformOf(viewer)).not.toBe(before.transform);
    expect(await board.evaluate((el) => el.style.backgroundPosition)).not.toBe(before.dots);

    // A plain wheel scrolls the page and leaves the zoom alone.
    const zoomBefore = await zoomOf(viewer);
    const scrollBefore = await page.evaluate(() => document.scrollingElement?.scrollTop ?? 0);
    await page.mouse.move(box.x + 200, box.y + 100);
    await page.mouse.wheel(0, 120);
    await page.waitForTimeout(150);
    expect(await zoomOf(viewer)).toBe(zoomBefore);
    expect(await board.evaluate((el) => el.getBoundingClientRect().top)).not.toBe(box.y); // the page (or the transcript) moved

    // Ctrl + wheel zooms.
    const box2 = (await board.boundingBox())!;
    await page.mouse.move(box2.x + 200, box2.y + 100);
    await page.keyboard.down("Control");
    await page.mouse.wheel(0, -200);
    await page.keyboard.up("Control");
    await expect.poll(() => zoomOf(viewer)).toBeGreaterThan(zoomBefore);
    void scrollBefore;

    // Double click fits again.
    await board.dblclick({ position: { x: 300, y: 80 } });
    await expect.poll(() => zoomOf(viewer)).toBe(fitted);

    // Buttons.
    await viewer.getByRole("button", { name: "Zoom in" }).click();
    expect(await zoomOf(viewer)).toBeGreaterThan(fitted);
    await viewer.getByRole("button", { name: "Fit to view" }).click();
    expect(await zoomOf(viewer)).toBe(fitted);

    // Source and Copy.
    await viewer.getByRole("button", { name: "Show source" }).click();
    await expect(viewer.getByTestId("diagram-source")).toContainText("flowchart LR");
    await viewer.getByRole("button", { name: "Copy code" }).first().click();
    await expect(viewer.getByRole("button", { name: "Copied" }).first()).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("flowchart LR\n  A[Mono request] --> B{Risk?}\n  B -- safe --> C[Auto act]\n  B -- risky --> D[Ask user]\n  D -- allow --> E[Commit]\n  D -- deny --> F[Stop]");
    await viewer.getByRole("button", { name: "Show diagram" }).click();

    // Fullscreen fills the window and Esc leaves it.
    await viewer.getByRole("button", { name: "Fullscreen" }).click();
    const overlay = page.getByTestId("diagram-fullscreen");
    await expect(overlay).toBeVisible();
    const overlayBox = (await overlay.boundingBox())!;
    expect(overlayBox.width).toBe(1100);
    await expect(overlay.getByRole("button", { name: "Exit fullscreen" })).toBeVisible();
    await shot("desktop-mermaid-fullscreen");
    await page.keyboard.press("Escape");
    await expect(overlay).toHaveCount(0);
  });

  test("desktop: a mermaid diagram that is not a flowchart stays source, and the visuals are drawn", async () => {
    await open({ viewport: { width: 1100, height: 900 } }, "light");
    const fallback = page.getByTestId("mermaid-fallback");
    await expect(fallback).toContainText("sequenceDiagram");
    await expect(fallback).toContainText("Shown as source");
    // visual.flow in the same viewer, with its edge labels, and no Source button.
    const flow = page.getByTestId("visual-flow");
    await expect(flow.locator("[data-flow-node]")).toHaveCount(5);
    await expect(flow.getByText("safe", { exact: true })).toBeVisible();
    await expect(flow.getByRole("button", { name: "Show source" })).toHaveCount(0);
    // Canvas labels and the stat with a chart.
    const canvas = page.locator("svg[data-visual='canvas']");
    await expect(canvas.getByText("Centered")).toBeVisible();
    await expect(canvas.locator("rect[data-chip]")).toHaveCount(1);
    await expect(page.getByText("42", { exact: true })).toBeVisible();
    await expect(page.getByRole("img", { name: "bar chart" })).toBeVisible();
    // Nothing in the page is a script, an image of the reply, or an injected element.
    expect(await page.locator("[data-testid='transcript-visual'] script, [data-testid='transcript-visual'] img").count()).toBe(0);
    await flow.scrollIntoViewIfNeeded();
    await shot("desktop-visuals-light");
  });

  test("phone: the viewer fits 390 px, pinches, double taps, drags, and the page does not scroll sideways", async () => {
    await open({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true, deviceScaleFactor: 2 });
    const viewer = page.getByTestId("mermaid-diagram");
    await viewer.scrollIntoViewIfNeeded();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    const panel = (await viewer.boundingBox())!;
    expect(panel.x).toBeGreaterThanOrEqual(0);
    expect(panel.x + panel.width).toBeLessThanOrEqual(390);
    // Every toolbar control is on screen and a finger can hit it.
    for (const name of ["Zoom out", "Zoom in", "Fit to view", "Fullscreen", "Show source", "Copy code"]) {
      const button = (await viewer.getByRole("button", { name }).boundingBox())!;
      expect(button.x + button.width, name).toBeLessThanOrEqual(390);
      expect(button.height, name).toBeGreaterThanOrEqual(34);
    }
    await shot("phone-mermaid");
    const fitted = await zoomOf(viewer);
    const board = viewer.getByTestId("diagram-board");
    const box = (await board.boundingBox())!;
    const cx = box.x + box.width / 2;
    const cy = box.y + box.height / 2;

    // Pinch out with two fingers.
    await touch(page, [
      { type: "touchStart", points: [{ x: cx - 30, y: cy, id: 1 }, { x: cx + 30, y: cy, id: 2 }] },
      ...[1, 2, 3, 4, 5, 6].map((i) => ({ type: "touchMove" as const, wait: 20, points: [{ x: cx - 30 - i * 12, y: cy, id: 1 }, { x: cx + 30 + i * 12, y: cy, id: 2 }] })),
      { type: "touchEnd", points: [] },
    ]);
    await expect.poll(() => zoomOf(viewer)).toBeGreaterThan(fitted);

    // One finger drags.
    const beforeDrag = await transformOf(viewer);
    await touch(page, [
      { type: "touchStart", points: [{ x: cx, y: cy, id: 1 }] },
      ...[1, 2, 3, 4].map((i) => ({ type: "touchMove" as const, wait: 20, points: [{ x: cx + i * 10, y: cy + i * 5, id: 1 }] })),
      { type: "touchEnd", points: [] },
    ]);
    expect(await transformOf(viewer)).not.toBe(beforeDrag);

    // Double tap fits.
    const tap = async () => {
      await touch(page, [{ type: "touchStart", points: [{ x: cx, y: cy, id: 1 }] }, { type: "touchEnd", points: [], wait: 30 }]);
    };
    await tap();
    await page.waitForTimeout(80);
    await tap();
    await expect.poll(() => zoomOf(viewer)).toBe(fitted);

    // Fullscreen covers the phone and Fit keeps the diagram inside it.
    await viewer.getByRole("button", { name: "Fullscreen" }).tap();
    const overlay = page.getByTestId("diagram-fullscreen");
    await expect(overlay).toBeVisible();
    const full = (await overlay.boundingBox())!;
    expect([full.width, full.height]).toEqual([390, 844]);
    await shot("phone-fullscreen");
    await overlay.getByRole("button", { name: "Exit fullscreen" }).tap();
    await expect(overlay).toHaveCount(0);
  });
});
