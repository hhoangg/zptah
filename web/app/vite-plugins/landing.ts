// Builds the landing page (web/landing, plain Node build) into the client output of every `vite build`, so each place that
// builds the app (the app build script, the e2e harness, CI, the official deploy) ends up with `<outDir>/landing/`, which
// the Worker serves at `/` without a session cookie and at `/home` (worker/src/landing.ts).
import path from "node:path";
import { pathToFileURL } from "node:url";
import type { Plugin } from "vite";
import { resolveRpOrigin } from "./headers.ts";

interface LandingBuild {
  build: (options: { target: "prod" | "beta"; out: string; site: string }) => Promise<unknown>;
}

/** `wranglerPath` is the config whose `vars.RP_ORIGIN` (or MONOPTAH_RP_ORIGIN) names the host: canonical, sitemap and og tags use it. */
export function landingPlugin(wranglerPath: string): Plugin {
  return {
    name: "zptah-landing",
    apply: "build",
    async writeBundle(options) {
      // The Cloudflare plugin also builds a Worker environment; only the client output gets the page.
      if (this.environment.name !== "client" || !options.dir) return;
      const script = path.resolve(import.meta.dirname, "..", "..", "landing", "build.mjs");
      const { build } = (await import(pathToFileURL(script).href)) as LandingBuild;
      await build({ target: "prod", out: path.resolve(options.dir), site: resolveRpOrigin(path.resolve(wranglerPath)).replace(/\/$/, "") });
    },
  };
}
