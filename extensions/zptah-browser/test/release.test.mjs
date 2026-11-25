import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { build } from "../scripts/build.mjs";
import { checkKnownBuild, checkRelease, configProtocol, packageHash, release } from "../scripts/release.mjs";

const tmp = () => mkdtempSync(join(tmpdir(), "ext-release-"));
const KNOWN_RS = new URL("../../../crates/monocode-core/src/mono/browser/links/known_builds.rs", import.meta.url);

test("packageHash follows the app's hash_extension_dir (independent vector)", () => {
  const dir = tmp();
  try {
    mkdirSync(join(dir, "src"));
    mkdirSync(join(dir, "_metadata"));
    writeFileSync(join(dir, "manifest.json"), '{"name":"x","version":"1.0.0","key":"K"}\n');
    writeFileSync(join(dir, "src/a.js"), "run();");
    writeFileSync(join(dir, "_metadata/ignored.json"), "{}");
    // Computed with python (sha256 over "path NUL sha256(content)", manifest without key, compact JSON).
    assert.equal(packageHash(dir), "8e12cc376974dd2e0e6a190e407081c245f920792e8cf53629c85350af3453c6");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("checkRelease ties the tag, the manifest and the protocol major together", () => {
  assert.deepEqual(checkRelease("ext-v1.2.3", { version: "1.2.3" }, 1), { version: "1.2.3", protocol: 1 });
  assert.throws(() => checkRelease("ext-v1.2.3", { version: "1.2.4" }, 1), /does not match manifest/);
  assert.throws(() => checkRelease("ext-v2.0.0", { version: "2.0.0" }, 1), /protocol 1/);
  assert.throws(() => checkRelease("ext-v1.0.0-rc1", { version: "1.0.0" }, 1), /ext-vX\.Y\.Z/);
  assert.throws(() => checkRelease("v1.0.0", { version: "1.0.0" }, 1), /ext-vX\.Y\.Z/);
  assert.throws(() => checkRelease("ext-v1.0.0", { version: "1.0.0", version_name: "1.0.0-dev" }, 1), /version_name/);
});

test("checkKnownBuild needs the version listed and a matching hash when one is recorded", () => {
  const hash = "a".repeat(64);
  const src = `pub(crate) const KNOWN_BUILDS: &[KnownBuild] = &[KnownBuild {\n    version: "1.0.0",\n    sha256: None,\n}, KnownBuild { version: "1.1.0", sha256: Some("${hash}") }];`;
  checkKnownBuild(src, "1.0.0", "b".repeat(64));
  checkKnownBuild(src, "1.1.0", hash);
  assert.throws(() => checkKnownBuild(src, "1.1.0", "b".repeat(64)), /another hash/);
  assert.throws(() => checkKnownBuild(src, "1.2.0", hash), /does not list version 1.2.0/);
});

test("the real known_builds.rs lists the manifest version", () => {
  const manifest = JSON.parse(readFileSync(new URL("../manifest.json", import.meta.url), "utf8"));
  checkKnownBuild(readFileSync(KNOWN_RS, "utf8"), manifest.version, "0".repeat(64));
});

test("config.js protocol is the manifest major", () => {
  const manifest = JSON.parse(readFileSync(new URL("../manifest.json", import.meta.url), "utf8"));
  assert.equal(configProtocol(), Number(manifest.version.split(".")[0]));
});

test("release builds a deterministic zip and extension-release.json", { skip: !hasZip() }, () => {
  const root = tmp();
  try {
    const dist = join(root, "dist");
    build({ outRoot: dist });
    const manifest = JSON.parse(readFileSync(new URL("../manifest.json", import.meta.url), "utf8"));
    const tag = `ext-v${manifest.version}`;
    const a = release({ tag, out: join(root, "a"), dist: join(dist, "store") });
    const b = release({ tag, out: join(root, "b"), dist: join(dist, "store") });
    assert.deepEqual(a, b);
    assert.equal(a.version, manifest.version);
    assert.match(a.sha256, /^[0-9a-f]{64}$/);
    assert.equal(a.package_sha256, packageHash(join(dist, "store")));
    const json = JSON.parse(readFileSync(join(root, "a/extension-release.json"), "utf8"));
    assert.deepEqual(json, a);
    const listing = execFileSync("unzip", ["-Z1", join(root, "a", a.file)], { encoding: "utf8" });
    assert.match(listing, /^manifest\.json$/m);
    assert.throws(() => release({ tag: "ext-v9.9.9", out: join(root, "c"), dist: join(dist, "store") }), /does not match/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

function hasZip() {
  try {
    execFileSync("zip", ["-v"], { stdio: "ignore" });
    execFileSync("unzip", ["-v"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}
