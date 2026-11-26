// Mermaid blocks of a reply drawn with the official `mermaid` library under the production Content-Security-Policy and
// Trusted Types: the page is a production build of the dev preview (an in-browser mock desktop, see lib/preview-server.ts)
// served with the Worker's real security headers. One reply carries a flowchart, a sequence diagram and a block that does
// not parse. The flowchart and the sequence diagram must be drawn as images in the shared viewer, the broken one must stay
// source with a note, and nothing may trip the CSP or the Trusted Types (a `securitypolicyviolation` event or a console error).
import { chromium, expect, test, type Browser, type BrowserContext, type Page } from "@playwright/test";
import { startPreviewServer, type PreviewServer } from "../lib/preview-server.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const SESSION = (theme: string, session = "sess-visuals") => `/?to=/d/${DESKTOP_ID}/s/${session}&theme=${theme}`;

/** The blocks of the `sess-mermaid-types` reply, in order (web/mock-desktop/src/fixtures.ts). Every one is drawn as an image. */
const TYPES = [
  "flowchart",
  "sequenceDiagram",
  "classDiagram",
  "stateDiagram",
  "erDiagram",
  "gantt",
  "pie",
  "gitGraph",
  "mindmap",
  "architecture with icons",
  // Math: with SVG labels (htmlLabels is off, see security.md 8.2) the $$ text is drawn as plain text, not typeset.
  "flowchart with a $$ label",
  // An icon shape whose pack is not installed: drawn with mermaid's placeholder.
  "flowchart icon shape",
  // `%%{init: {"htmlLabels": true}}%%` cannot switch the labels back to HTML: the directive is ignored.
  "init directive htmlLabels",
];

declare global {
  interface Window {
    __violations?: string[];
  }
}

test.describe("mermaid under the production CSP and Trusted Types", () => {
  test.describe.configure({ mode: "serial" });
  let server: PreviewServer;
  let browser: Browser;
  let context: BrowserContext;
  let page: Page;
  const problems: string[] = [];

  test.beforeAll(async () => {
    test.setTimeout(180_000);
    server = await startPreviewServer();
    browser = await chromium.launch({ headless: true });
  });

  test.afterAll(async () => {
    await browser?.close();
    await server?.stop();
  });

  test.afterEach(async () => {
    await context?.close();
  });

  async function open(theme: string, session = "sess-visuals"): Promise<void> {
    problems.length = 0;
    context = await browser.newContext({ viewport: { width: 1100, height: 900 } });
    await context.addInitScript(() => {
      window.__violations = [];
      document.addEventListener("securitypolicyviolation", (event) => {
        window.__violations?.push(`${event.violatedDirective}: ${event.blockedURI} ${event.sample}`);
      });
    });
    page = await context.newPage();
    page.on("console", (message) => {
      if (message.type() === "error" || message.type() === "warning") problems.push(`${message.type()}: ${message.text()}`);
    });
    page.on("pageerror", (error) => problems.push(`pageerror: ${error.message}`));
    const response = await page.goto(`${server.base}${SESSION(theme, session)}`);
    // The served page really carries the production policy, so the checks below are made under it.
    expect(response?.headers()["content-security-policy"]).toBe(server.csp);
    expect(server.csp).toContain("require-trusted-types-for 'script'");
    expect(await page.evaluate(() => typeof (window as unknown as { trustedTypes?: unknown }).trustedTypes)).toBe("object");
  }

  test("a flowchart and a sequence diagram of one reply are both drawn, a broken one stays source", async () => {
    await open("dark");
    const viewers = page.getByTestId("mermaid-diagram");
    await expect(viewers).toHaveCount(2, { timeout: 60_000 });
    for (const index of [0, 1]) {
      const image = viewers.nth(index).locator("image[data-testid='mermaid-image']");
      await expect(image).toHaveCount(1);
      expect(await image.getAttribute("href")).toMatch(/^blob:/);
      // The browser decoded it (a drawing it refuses has no size) and it has a real size.
      const size = await image.evaluate(
        (el) =>
          new Promise<[number, number]>((resolve) => {
            const probe = new Image();
            probe.onload = () => resolve([probe.naturalWidth, probe.naturalHeight]);
            probe.onerror = () => resolve([0, 0]);
            probe.src = el.getAttribute("href") ?? "";
          }),
      );
      expect(size[0]).toBeGreaterThan(50);
      expect(size[1]).toBeGreaterThan(50);
      // The image is on the board with the viewer's toolbar around it.
      await expect(viewers.nth(index).getByRole("button", { name: "Fit to view" })).toBeVisible();
      expect((await image.boundingBox())!.width).toBeGreaterThan(40);
    }
    // The sequence diagram is the second block and is not the fallback.
    await expect(viewers.nth(1).getByRole("button", { name: "Show source" })).toBeVisible();
    await viewers.nth(1).getByRole("button", { name: "Show source" }).click();
    await expect(viewers.nth(1).getByTestId("diagram-source")).toContainText("sequenceDiagram");
    // The block that does not parse shows its source and a short note, and is the only fallback.
    const fallback = page.getByTestId("mermaid-fallback");
    await expect(fallback).toHaveCount(1);
    await expect(fallback).toContainText("flowchart LR");
    await expect(fallback).toContainText(/Shown as source: could not draw this diagram \(.+\)\./);
    // Mermaid leaves nothing behind in the page: no scratch element, no markup of its own.
    expect(await page.locator("body > [id^='dzptah-mermaid'], body > [id^='zptah-mermaid'], #root style, #root script, #root foreignObject").count()).toBe(0);
    expect(await page.evaluate(() => window.__violations)).toEqual([]);
    expect(problems.filter((line) => !line.includes("Failed to load resource"))).toEqual([]);
  });

  test("a light theme draws too, and the Trusted Types default policy still refuses HTML afterwards", async () => {
    await open("light");
    await expect(page.getByTestId("mermaid-diagram")).toHaveCount(2, { timeout: 60_000 });
    const outcome = await page.evaluate(() => {
      const probe = document.createElement("div");
      try {
        // eslint-disable-next-line no-restricted-properties -- this test proves the sink is still refused after a render
        probe.innerHTML = "<img src=x>";
        return "allowed";
      } catch (error) {
        return error instanceof Error ? error.message : "refused";
      }
    });
    expect(outcome).toContain("HTML sinks are disabled");
    expect(await page.evaluate(() => window.__violations?.filter((line) => !line.startsWith("require-trusted-types-for")))).toEqual([]);
  });

  test("every diagram type of the reply is drawn as an image under the production policy", async () => {
    await open("dark", "sess-mermaid-types");
    const viewers = page.getByTestId("mermaid-diagram");
    await expect(viewers).toHaveCount(TYPES.length, { timeout: 90_000 });
    await expect(page.getByTestId("mermaid-fallback")).toHaveCount(0);
    for (const [index, type] of TYPES.entries()) {
      const image = viewers.nth(index).locator("image[data-testid='mermaid-image']");
      const href = (await image.getAttribute("href")) ?? "";
      expect(href, type).toMatch(/^blob:/);
      // Decoded by the browser, with a real size; the colours drawn are read back to see the architecture icons are in it.
      const probe = await image.evaluate(
        (el) =>
          new Promise<{ width: number; height: number; blue: number }>((resolve) => {
            const picture = new Image();
            picture.onload = () => {
              const canvas = document.createElement("canvas");
              canvas.width = picture.naturalWidth;
              canvas.height = picture.naturalHeight;
              const context = canvas.getContext("2d");
              context?.drawImage(picture, 0, 0);
              const data = context?.getImageData(0, 0, canvas.width, canvas.height).data ?? new Uint8ClampedArray();
              let blue = 0;
              // The architecture icons are filled #087ebf (8, 126, 191).
              for (let i = 0; i < data.length; i += 4) if (Math.abs((data[i] ?? 0) - 8) < 6 && Math.abs((data[i + 1] ?? 0) - 126) < 6 && Math.abs((data[i + 2] ?? 0) - 191) < 6 && (data[i + 3] ?? 0) > 250) blue++;
              resolve({ width: picture.naturalWidth, height: picture.naturalHeight, blue });
            };
            picture.onerror = () => resolve({ width: 0, height: 0, blue: 0 });
            picture.src = el.getAttribute("href") ?? "";
          }),
      );
      expect(probe.width, type).toBeGreaterThan(30);
      expect(probe.height, type).toBeGreaterThan(30);
      if (type === "architecture with icons") expect(probe.blue, "icon pixels in the architecture diagram").toBeGreaterThan(500);
    }
    expect(await page.evaluate(() => window.__violations)).toEqual([]);
    expect(problems.filter((line) => !line.includes("Failed to load resource"))).toEqual([]);
  });
});
