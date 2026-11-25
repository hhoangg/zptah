/* global console, process */
// Creates a placeholder `web/app/dist/client/index.html` when the app has not been built, so that
// `wrangler dev` and the Workers tests can start (the assets binding needs its directory).
// `app/dist` is git-ignored (web/.gitignore) and owned by the app build, which overwrites it.
// Never replaces an existing build.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "..", "..", "app", "dist", "client");
const index = join(dist, "index.html");
const landing = join(dist, "landing", "index.html");

// The real landing page carries the same `<meta name="zptah-page" content="landing">` (web/landing/src/html/page.mjs), so
// worker/test/landing.test.ts tells the landing from the SPA in both cases.
const LANDING_PLACEHOLDER_MARKER = "zptah landing placeholder";

const SPA_PLACEHOLDER_MARKER = "Monoptah web placeholder";

export function ensureAssets() {
  let created = false;
  const hadIndex = existsSync(index);
  if (!hadIndex) {
    mkdirSync(dist, { recursive: true });
    writeFileSync(
      index,
      `<!doctype html>\n<html lang="en"><head><meta charset="utf-8"><title>Monoptah</title></head>\n<body>${SPA_PLACEHOLDER_MARKER}: build web/app to replace it.</body></html>\n`,
    );
    created = true;
  }
  // The landing placeholder only goes where the whole dist is a placeholder: never into a real build (whose landing
  // page comes from vite-plugins/landing.ts), so a broken build is not hidden by a placeholder.
  const placeholderDist = !hadIndex || readFileSync(index, "utf8").includes(SPA_PLACEHOLDER_MARKER);
  if (placeholderDist && !existsSync(landing)) {
    mkdirSync(dirname(landing), { recursive: true });
    writeFileSync(
      landing,
      `<!doctype html>\n<html lang="en"><head><meta charset="utf-8"><meta name="zptah-page" content="landing"><title>zptah</title></head>\n<body>${LANDING_PLACEHOLDER_MARKER}: the app build replaces it.</body></html>\n`,
    );
    created = true;
  }
  return created;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  console.log(ensureAssets() ? `created placeholder assets in ${dist}` : "app/dist/client already has an index.html and a landing page");
}
