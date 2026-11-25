// @ts-nocheck - mocks and regex matches are untyped.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (p) => readFileSync(new URL(p, import.meta.url), "utf8");
const manifest = JSON.parse(read("../manifest.json"));
const justifications = read("../store/justifications.md");
const listing = read("../store/listing.md");

test("every manifest permission has a justification row", () => {
  const all = [...manifest.permissions, ...manifest.optional_permissions, ...manifest.host_permissions];
  for (const p of all) assert.ok(justifications.includes(`| \`${p}\``), `justification for ${p}`);
});

test("the listing has both languages, the single purpose and a short description within 132 characters", () => {
  assert.match(listing, /## Single purpose statement/);
  assert.match(listing, /## Long description, English/);
  assert.match(listing, /## Long description, Vietnamese/);
  assert.match(listing, /https:\/\/zptah\.app\/privacy/);
  const shorts = [...listing.matchAll(/^> (.+)$/gm)].map((m) => m[1]).slice(0, 2);
  assert.equal(shorts.length, 2);
  for (const s of shorts) assert.ok([...s].length <= 132, s);
});

test("store docs contain no em or en dash", () => {
  const dash = new RegExp("[" + String.fromCharCode(0x2013, 0x2014) + "]");
  for (const p of ["../store/listing.md", "../store/justifications.md", "../store/screenshots/README.md"]) assert.doesNotMatch(read(p), dash, p);
});
