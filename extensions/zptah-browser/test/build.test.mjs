import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { build } from "../scripts/build.mjs";

/** @param {string} dir @returns {string} */
function digest(dir) {
  const h = createHash("sha256");
  /** @param {string} d */
  const walk = (d) => {
    for (const n of readdirSync(d).sort()) {
      const p = join(d, n);
      if (statSync(p).isDirectory()) walk(p);
      else h.update(p.slice(dir.length)).update("\0").update(readFileSync(p));
    }
  };
  walk(dir);
  return h.digest("hex");
}

/** @param {string} root */
const manifest = (root) => JSON.parse(readFileSync(join(root, "manifest.json"), "utf8"));

test("build is deterministic and the variants differ as specified", () => {
  const a = mkdtempSync(join(tmpdir(), "zptah-ext-a-"));
  const b = mkdtempSync(join(tmpdir(), "zptah-ext-b-"));
  try {
    build({ outRoot: a });
    build({ outRoot: b });
    assert.equal(digest(a), digest(b));

    const store = manifest(join(a, "store"));
    const dev = manifest(join(a, "dev"));
    assert.equal("key" in store, false);
    assert.equal("update_url" in store, false);
    assert.equal(typeof dev.key, "string");
    assert.match(readFileSync(join(a, "store/src/worker/config.js"), "utf8"), /"app\.zptah\.browser_host"/);
    assert.match(readFileSync(join(a, "dev/src/worker/config.js"), "utf8"), /"app\.zptah\.browser_host_dev"/);
    assert.doesNotMatch(readFileSync(join(a, "store/src/worker/config.js"), "utf8"), /_dev/);
  } finally {
    rmSync(a, { recursive: true, force: true });
    rmSync(b, { recursive: true, force: true });
  }
});

test("content script is one IIFE without import or export", () => {
  const out = mkdtempSync(join(tmpdir(), "zptah-ext-c-"));
  try {
    build({ outRoot: out });
    const text = readFileSync(join(out, "store/src/content/index.js"), "utf8");
    assert.ok(text.startsWith("(() => {"));
    assert.ok(text.trimEnd().endsWith("})();"));
    assert.doesNotMatch(text, /^\s*(?:import|export)\b/m);
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
});

test("dev id derived from the dev key is stable", () => {
  const dev = JSON.parse(readFileSync(new URL("../manifest.dev.json", import.meta.url), "utf8"));
  const der = Buffer.from(dev.key, "base64");
  const hex = createHash("sha256").update(der).digest("hex").slice(0, 32);
  const id = [...hex].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join("");
  assert.match(id, /^[a-p]{32}$/);
  assert.equal(id, "bmmfejkhajglhijajgdgfckhonddijjh");
});
