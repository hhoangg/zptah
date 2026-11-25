// No-dependency build: copies sources, bundles the content script into one IIFE, writes
// dist/store and dist/dev, and fails on forbidden APIs (spec 4.2, 6.6). Deterministic: no
// timestamps, sorted traversal, stable JSON.
import { cpSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { scanSource, scanTree } from "./scan.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

export const HOST_NAME = "app.zptah.browser_host";
export const DEV_HOST_NAME = "app.zptah.browser_host_dev";

/** @param {string} p */
const readJson = (p) => JSON.parse(readFileSync(p, "utf8"));

/**
 * Concatenate src/content/*.js (index.js last) into one IIFE. Content files are plain
 * scripts: import and export are rejected because the result is not a module.
 * @param {string} dir
 */
export function bundleContent(dir) {
  const names = readdirSync(dir)
    .filter((n) => n.endsWith(".js"))
    .sort((a, b) => (a === "index.js" ? 1 : b === "index.js" ? -1 : a < b ? -1 : 1));
  const parts = names.map((n) => {
    const text = readFileSync(join(dir, n), "utf8");
    if (/^\s*(?:import|export)\b/m.test(text)) {
      throw new Error(`${n}: content scripts must not use import or export`);
    }
    return `// ${n}\n${text.trimEnd()}\n`;
  });
  return `(() => {\n"use strict";\n${parts.join("\n")}})();\n`;
}

/**
 * @param {"store" | "dev"} variant
 * @param {string} outDir
 */
function buildVariant(variant, outDir) {
  rmSync(outDir, { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  cpSync(join(ROOT, "src/worker"), join(outDir, "src/worker"), { recursive: true });
  cpSync(join(ROOT, "src/options"), join(outDir, "src/options"), { recursive: true });
  mkdirSync(join(outDir, "src/content"), { recursive: true });
  writeFileSync(join(outDir, "src/content/index.js"), bundleContent(join(ROOT, "src/content")));

  let manifest = readJson(join(ROOT, "manifest.json"));
  if (variant === "dev") {
    manifest = { ...manifest, ...readJson(join(ROOT, "manifest.dev.json")) };
    const cfg = join(outDir, "src/worker/config.js");
    const text = readFileSync(cfg, "utf8");
    if (!text.includes(`"${HOST_NAME}"`)) throw new Error("config.js does not hold the store host name");
    writeFileSync(cfg, text.replace(`"${HOST_NAME}"`, `"${DEV_HOST_NAME}"`));
  } else if ("key" in manifest || "update_url" in manifest) {
    throw new Error("the store manifest must not have key or update_url");
  }
  writeFileSync(join(outDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

  const problems = scanTree(outDir, { store: variant === "store" });
  if (problems.length > 0) {
    throw new Error(`forbidden API scan failed (${variant}):\n${problems.join("\n")}`);
  }
}

/**
 * @param {{ outRoot?: string }} [opts]
 */
export function build(opts = {}) {
  const outRoot = opts.outRoot ?? join(ROOT, "dist");
  buildVariant("store", join(outRoot, "store"));
  buildVariant("dev", join(outRoot, "dev"));
  return outRoot;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(`built ${build()}`);
  } catch (err) {
    console.error(err instanceof Error ? err.message : err);
    process.exit(1);
  }
}

export { scanSource };
