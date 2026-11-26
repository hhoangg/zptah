// The controls of a native visual in a chat (tabs, button, toggle, select, slider, input) work on the web app, in Chromium
// (desktop) and in WebKit with an iPhone profile (project `webkit-mobile`, real touch taps). The page is the production
// build of the dev preview served with the Worker's real security headers and talks to an in-browser mock desktop whose
// `visual.dispatch` answers the next tree of the fixture visual of `sess-visuals` (web/mock-desktop/src/visual.ts), as the
// real desktop runs the program in its own sandbox. No relay, account or passkey is involved.
import { expect, test, type BrowserContext, type Locator, type Page } from "@playwright/test";
import { startPreviewServer, type PreviewServer } from "../lib/preview-server.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";

test.describe("controls of a visual in a chat", () => {
  test.describe.configure({ mode: "serial" });
  let server: PreviewServer;
  let context: BrowserContext;
  let page: Page;
  let touch = false;

  test.beforeAll(async () => {
    test.setTimeout(180_000);
    server = await startPreviewServer();
  });
  test.afterAll(async () => {
    await context?.close();
    await server?.stop();
  });

  /** A tap on a phone profile, a click elsewhere. */
  const press = (target: Locator) => (touch ? target.tap() : target.click());

  const figure = () => page.getByRole("figure", { name: "Interactive deploys" });
  /** The drawing of the first chart, as text, to see that it changed. */
  const drawing = () => figure().locator("svg").first().evaluate((svg) => new XMLSerializer().serializeToString(svg));
  const summary = () => figure().getByText(/^Clicks \d+ \//);

  test("setup: the session opens with the visual and its controls enabled", async ({ browser }, testInfo) => {
    touch = !!testInfo.project.use.hasTouch;
    context = await browser.newContext({ ...testInfo.project.use, baseURL: server.base, serviceWorkers: "block" });
    // WebKit upgrades every http://localhost sub-resource to https under `upgrade-insecure-requests`, which a plain http local
    // run cannot serve. Only that one directive is dropped from the response; every other directive stays in force.
    await context.route(
      (url) => url.protocol === "http:" && url.origin === server.base,
      async (route) => {
        const response = await route.fetch();
        const headers = response.headers();
        const csp = headers["content-security-policy"];
        if (csp) headers["content-security-policy"] = csp.split(";").map((part) => part.trim()).filter((part) => part !== "upgrade-insecure-requests").join("; ");
        await route.fulfill({ response, headers });
      },
    );
    page = await context.newPage();
    await page.goto(`/?to=/d/${DESKTOP_ID}/s/sess-visuals&theme=dark`);
    await expect(figure()).toBeVisible({ timeout: 60_000 });
    await figure().scrollIntoViewIfNeeded();
    await expect(figure().getByRole("tab")).toHaveCount(3);
    for (const tab of await figure().getByRole("tab").all()) await expect(tab).toBeEnabled();
    await expect(figure().getByText("Open on desktop to interact")).toHaveCount(0);
    await expect(figure().getByRole("tab", { name: "Bar" })).toHaveAttribute("aria-selected", "true");
  });

  test("a tapped tab switches the chart, and the next tap works on the tree that came back", async () => {
    const tabs = figure().getByRole("tab");
    // The tab is what is on top at its center: no fade, composer or scroller swallows the tap.
    const box = (await tabs.nth(2).boundingBox())!;
    expect(box.height).toBeGreaterThanOrEqual(touch ? 40 : 24);
    const hit = await page.evaluate(([x, y]) => document.elementFromPoint(x!, y!)?.closest("[role='tab']")?.textContent ?? "", [box.x + box.width / 2, box.y + box.height / 2]);
    expect(hit).toBe("Area");
    const bar = await drawing();
    await press(figure().getByRole("tab", { name: "Area" }));
    await expect(figure().getByRole("tab", { name: "Area" })).toHaveAttribute("aria-selected", "true");
    await expect(figure().getByRole("tab", { name: "Bar" })).toHaveAttribute("aria-selected", "false");
    await expect.poll(drawing).not.toBe(bar);
    const area = await drawing();
    await press(figure().getByRole("tab", { name: "Line" }));
    await expect(figure().getByRole("tab", { name: "Line" })).toHaveAttribute("aria-selected", "true");
    await expect.poll(drawing).not.toBe(area);
  });

  test("a button, a toggle and a select fire", async () => {
    await press(figure().getByRole("button", { name: "Add one" }));
    await expect(summary()).toContainText("Clicks 1 /");
    await press(figure().getByRole("button", { name: "Add one" }));
    await expect(summary()).toContainText("Clicks 2 /");
    const toggle = figure().getByRole("switch", { name: "Notify" });
    await expect(toggle).toHaveAttribute("aria-checked", "false");
    await press(toggle);
    await expect(toggle).toHaveAttribute("aria-checked", "true");
    await expect(summary()).toContainText("Notify on");
    await figure().getByRole("combobox", { name: "Range" }).selectOption({ label: "Month" });
    await expect(summary()).toContainText("Month");
  });

  test("a slider and an input fire", async () => {
    const slider = figure().getByRole("slider", { name: "Level" });
    await slider.focus();
    await page.keyboard.press("ArrowRight");
    await expect(summary()).toContainText("Level 31");
    await page.keyboard.press("End");
    await expect(summary()).toContainText("Level 100");
    const note = figure().getByRole("textbox", { name: "Note" });
    await note.fill("hello");
    await page.keyboard.press("Enter");
    await expect(summary()).toContainText("Note hello");
    await expect(figure().getByRole("alert")).toHaveCount(0);
  });
});
