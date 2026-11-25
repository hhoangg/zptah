// `/` is the landing page without a session cookie and the app with one; `/home` is always the landing page; the
// installed app's start_url (`/app`) goes to /login when signed out. The Worker decides (web/worker/src/landing.ts).
import { test, expect } from "../lib/fixtures.ts";
import { origin } from "../lib/env.ts";

test.describe("landing page routing", () => {
  test("/ without a cookie is the landing page, with working app links, and it throws no console error", async ({ browser }) => {
    const context = await browser.newContext({ baseURL: origin() });
    const page = await context.newPage();
    const problems: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error") problems.push(message.text());
    });
    page.on("pageerror", (error) => problems.push(error.message));
    await page.goto("/");
    await expect(page.getByRole("heading", { level: 1, name: "Every coding agent. One window." })).toBeVisible();
    await expect(page.getByRole("link", { name: "Sign in" })).toHaveAttribute("href", "/login");
    await expect(page.getByRole("link", { name: "Get started" })).toHaveAttribute("href", "/register");
    await expect(page.getByRole("link", { name: "Open web app" })).toHaveAttribute("href", "/login");
    await page.goto("/home");
    await expect(page.getByRole("heading", { level: 1, name: "Every coding agent. One window." })).toBeVisible();
    expect(problems).toEqual([]);
    await context.close();
  });

  test("/app without a session goes to the sign-in page", async ({ browser }) => {
    const context = await browser.newContext({ baseURL: origin() });
    const page = await context.newPage();
    await page.goto("/app");
    await expect(page).toHaveURL(/\/login/);
    await context.close();
  });

  test("the manifest starts at /app", async ({ request }) => {
    const manifest = (await (await request.get(`${origin()}/manifest.webmanifest`)).json()) as { start_url: string; scope: string };
    expect(manifest.start_url).toBe("/app");
    expect(manifest.scope).toBe("/");
  });

  test("/ with a session cookie is the app, not the landing page (the Worker only checks that the cookie is present)", async ({ browser }) => {
    const context = await browser.newContext({ baseURL: origin() });
    await context.addCookies([{ name: "__Host-mc_session", value: "present-but-not-valid", domain: new URL(origin()).hostname, path: "/", httpOnly: true, secure: true }]);
    const page = await context.newPage();
    await page.goto("/");
    // The app's own auth guard rejects the made-up session and sends it to /login; the landing page would stay on `/`.
    await expect(page).toHaveURL(/\/login/);
    await expect(page.getByRole("heading", { level: 1, name: "Every coding agent. One window." })).toHaveCount(0);
    await context.close();
  });

  test("with the service worker installed, / and /?utm=x for a signed-out visitor are still the landing page", async ({ browser }) => {
    const context = await browser.newContext({ baseURL: origin() });
    const page = await context.newPage();
    await page.goto("/login");
    const ready = await page.evaluate(async () => {
      if (!("serviceWorker" in navigator)) return false;
      const registration = await Promise.race([navigator.serviceWorker.ready.then(() => true), new Promise<boolean>((resolve) => setTimeout(() => resolve(false), 8000))]);
      return registration;
    });
    test.skip(!ready, "the app did not register a service worker in this harness");
    for (const target of ["/", "/?utm=x", "/home?ref=y"]) {
      await page.goto(target);
      await expect(page.getByRole("heading", { level: 1, name: "Every coding agent. One window." }), target).toBeVisible();
    }
    await context.close();
  });
});
