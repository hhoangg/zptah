import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { canonicalJson, canonicalizeText } from "../src/worker/canonical.js";

const vectors = JSON.parse(readFileSync(new URL("../shared/handshake-vectors.json", import.meta.url), "utf8"));

for (const [i, c] of vectors.canonical_cases.entries()) {
  test(`canonical case ${i}`, () => {
    assert.equal(canonicalizeText(c.json), c.canonical);
    assert.equal(canonicalJson(JSON.parse(c.json)), c.canonical);
  });
}

for (const text of vectors.canonical_rejects) {
  test(`canonical rejects ${text}`, () => {
    assert.throws(() => canonicalizeText(text));
  });
}

test("live values: floats, NaN, Infinity and unsafe integers are refused", () => {
  for (const v of [1.5, NaN, Infinity, 2 ** 53, -(2 ** 53), 1e300]) {
    assert.throws(() => canonicalJson({ n: v }), String(v));
  }
  assert.equal(canonicalJson({ n: 9007199254740991, m: -9007199254740991, z: -0 }), '{"m":-9007199254740991,"n":9007199254740991,"z":0}');
});

test("live values: undefined, non-ASCII keys and lone surrogates are refused", () => {
  assert.throws(() => canonicalJson({ a: undefined }));
  assert.throws(() => canonicalJson({ "é": 1 }));
  assert.throws(() => canonicalJson({ s: "\ud800" }));
  assert.throws(() => canonicalJson(new Date(0)));
});

test("number literals inside strings do not trip the integer check", () => {
  assert.equal(canonicalizeText('{"s":"1.0 and 1e2","n":3}'), '{"n":3,"s":"1.0 and 1e2"}');
  assert.equal(canonicalizeText('{"s":"quote \\" 1.5"}'), '{"s":"quote \\" 1.5"}');
});
