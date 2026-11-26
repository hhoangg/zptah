import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const manifest = JSON.parse(readFileSync(new URL("../manifest.json", import.meta.url), "utf8"));

// Permission snapshot (spec 13.2). Changing a permission is a store re-review: update this
// list on purpose, in the same change as the manifest.
test("permission snapshot", () => {
  assert.deepEqual(manifest.permissions, [
    "nativeMessaging",
    "tabs",
    "tabGroups",
    "scripting",
    "storage",
    "alarms",
    "webNavigation",
    "debugger",
  ]);
  // Chrome lists `debugger` under "cannot be optional": declared optional it is dropped and
  // permissions.request rejects. The user's switch in the options page gates its use instead.
  assert.equal("optional_permissions" in manifest, false);
  assert.deepEqual(manifest.host_permissions, ["<all_urls>"]);
  assert.equal(manifest.incognito, "not_allowed");
});

test("manifest has no key, no external access and no remote surface", () => {
  assert.equal("key" in manifest, false);
  assert.equal("update_url" in manifest, false);
  assert.equal("externally_connectable" in manifest, false);
  assert.equal("content_scripts" in manifest, false);
  assert.equal("web_accessible_resources" in manifest, false);
  assert.equal("action" in manifest && "default_popup" in manifest.action, false);
  assert.equal(manifest.manifest_version, 3);
  assert.equal(manifest.background.type, "module");
});

test("forbidden permissions are absent", () => {
  const all = [...manifest.permissions, ...(manifest.optional_permissions ?? [])];
  for (const p of [
    "identity", "downloads", "cookies", "history", "bookmarks", "webRequest",
    "declarativeNetRequest", "notifications", "sidePanel", "activeTab", "management",
    "clipboardRead", "clipboardWrite", "userScripts",
  ]) {
    assert.equal(all.includes(p), false, p);
  }
});

test("version major is the protocol major", () => {
  assert.match(manifest.version, /^1\.\d+\.\d+$/);
});

test("CSP is exactly script-src 'self'; object-src 'self'", () => {
  assert.deepEqual(manifest.content_security_policy, {
    extension_pages: "script-src 'self'; object-src 'self'",
  });
});

test("minimum Chrome version allows 30 s alarms", () => {
  assert.ok(Number(manifest.minimum_chrome_version) >= 120);
});
