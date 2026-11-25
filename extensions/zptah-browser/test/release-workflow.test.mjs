// @ts-nocheck - mocks and regex matches are untyped.
// Static checks of .github/workflows/extension-release.yml (spec 13.6). actionlint covers syntax
// where it is installed; these assertions keep the safety properties from being edited away.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const text = readFileSync(new URL("../../../.github/workflows/extension-release.yml", import.meta.url), "utf8");

/** Job blocks by name: from `  name:` at two-space indent to the next job. */
function jobs() {
  const body = text.slice(text.indexOf("\njobs:\n") + 7);
  const out = {};
  for (const m of body.matchAll(/^  ([a-z-]+):\n((?:(?:    .*|)\n?)*)/gm)) out[m[1]] = m[2];
  return out;
}

test("triggers: ext-v* tags and manual dispatch only", () => {
  assert.match(text, /on:\n  push:\n    tags:\n      - "ext-v\*"\n  workflow_dispatch:/);
  assert.doesNotMatch(text, /pull_request|branches:|schedule:/);
});

test("every job is gated to the fork and has a timeout", () => {
  const all = jobs();
  assert.deepEqual(Object.keys(all).sort(), ["build", "publish"]);
  for (const [name, block] of Object.entries(all)) {
    assert.match(block, /^    if: github\.repository == 'hhoangg\/zptah'$/m, `${name} gate`);
    assert.match(block, /^    timeout-minutes: \d+$/m, `${name} timeout`);
  }
});

test("only the publish job sees secrets, in the protected environment, after the build", () => {
  const { build, publish } = jobs();
  assert.doesNotMatch(build, /secrets\./);
  assert.doesNotMatch(build, /environment:/);
  assert.match(publish, /^    environment: chrome-web-store$/m);
  assert.match(publish, /^    needs: build$/m);
  assert.match(publish, /secrets\.CWS_SERVICE_ACCOUNT_KEY/);
  assert.match(publish, /secrets\.CWS_PUBLISHER_ID/);
  assert.match(publish, /--execute/);
  assert.match(publish, /STAGED_PUBLISH/);
  assert.doesNotMatch(publish, /id-token|attestations: write|contents: write/);
});

test("the build rebuilds from source, tests, checks the tag and attests", () => {
  const { build } = jobs();
  for (const step of ["npm run ext:test", "npm run ext:build", "scripts/release.mjs --tag", "actions/attest-build-provenance", "actions/upload-artifact"]) {
    assert.ok(build.includes(step), step);
  }
  assert.ok(build.indexOf("npm run ext:build") < build.indexOf("release.mjs"));
  assert.match(build, /id-token: write/);
});

test("the workflow never gets write access to contents and never creates a release or tag", () => {
  assert.doesNotMatch(text, /contents: write/);
  assert.doesNotMatch(text, /gh release|git push|git tag/);
});

test("no em or en dash", () => {
  assert.doesNotMatch(text, new RegExp("[" + String.fromCharCode(0x2013, 0x2014) + "]"));
});
