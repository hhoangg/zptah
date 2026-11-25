// @vitest-environment node
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { stripJsonc } from "./headers.ts";
import { landingPlugin } from "./landing.ts";

const wranglerPath = path.resolve(import.meta.dirname, "../../wrangler.jsonc");

describe("wrangler.jsonc routing of the landing page", () => {
  const config = JSON.parse(stripJsonc(readFileSync(wranglerPath, "utf8"))) as { assets: { run_worker_first: string[]; not_found_handling: string } };

  it("runs the Worker first for / , /home and /privacy (otherwise the assets layer answers them with the SPA), next to the API and the relay", () => {
    expect(config.assets.run_worker_first).toEqual(expect.arrayContaining(["/", "/home", "/home/", "/privacy", "/privacy/", "/api/*", "/relay/*"]));
  });

  it("keeps the SPA fallback for every other path", () => {
    expect(config.assets.not_found_handling).toBe("single-page-application");
  });
});

describe("landingPlugin", () => {
  const tempDirs: string[] = [];
  afterAll(() => {
    for (const dir of tempDirs) rmSync(dir, { recursive: true, force: true });
  });

  it("builds the page into the client output only, with the host of RP_ORIGIN", async () => {
    const plugin = landingPlugin(wranglerPath);
    const write = plugin.writeBundle as (this: unknown, options: { dir: string }) => Promise<void>;
    const workerOut = mkdtempSync(path.join(tmpdir(), "landing-worker-"));
    tempDirs.push(workerOut);
    await write.call({ environment: { name: "ssr" } }, { dir: workerOut });
    expect(() => readFileSync(path.join(workerOut, "landing", "index.html"))).toThrow();
    const clientOut = mkdtempSync(path.join(tmpdir(), "landing-client-"));
    tempDirs.push(clientOut);
    await write.call({ environment: { name: "client" } }, { dir: clientOut });
    expect(readFileSync(path.join(clientOut, "landing", "index.html"), "utf8")).toContain('<link rel="canonical" href="https://zptah.app/">');
    expect(readFileSync(path.join(clientOut, "sitemap.xml"), "utf8")).toContain("<loc>https://zptah.app/</loc>");
    expect(readFileSync(path.join(clientOut, "robots.txt"), "utf8")).toContain("Sitemap: https://zptah.app/sitemap.xml");
  });
});
