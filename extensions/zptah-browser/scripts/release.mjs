// @ts-nocheck - dependency-free CLI script; covered by its test file.
// Release helper for the extension (spec 13.5, 13.6). The workflow extension-release.yml calls it
// after `npm run ext:build`:
//
//   node extensions/zptah-browser/scripts/release.mjs --tag ext-v1.0.0 --out DIR
//
// It checks the tag against manifest.json and the protocol major, zips dist/store
// deterministically (sorted entries, fixed timestamps via `zip -X` and a normalized mtime) and
// writes DIR/zptah-browser-<version>.zip and DIR/extension-release.json:
//   { version, protocol, file, sha256, package_sha256 }
// `sha256` is the hash of the zip. `package_sha256` is the "known build" hash the app compiles in
// (crates/monocode-core/src/mono/browser/links/known_builds.rs, hash_extension_dir): every file in
// sorted path order, manifest.json without key and update_url, `_metadata/` skipped, each file
// contributing `path NUL sha256(content)`.
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, statSync, utimesSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const KNOWN_BUILDS_RS = "../../crates/monocode-core/src/mono/browser/links/known_builds.rs";

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

/** Relative POSIX paths of every file under dir (symlinks skipped, top-level `_metadata` skipped), sorted. */
export function listFiles(dir) {
  const out = [];
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      if (e.isSymbolicLink()) continue;
      const p = join(d, e.name);
      if (e.isDirectory()) {
        if (d === dir && e.name === "_metadata") continue;
        walk(p);
      } else if (e.isFile()) out.push(p.slice(dir.length + 1).split("\\").join("/"));
    }
  };
  walk(dir);
  return out.sort();
}

/**
 * manifest.json as the app hashes it: without key and update_url, object keys sorted recursively,
 * no whitespace. Independent of any parser's key order. Shared vectors: shared/manifest-canon-vectors.json
 * (also run by the Rust test in links/tests.rs).
 */
export function canonicalManifest(text) {
  const m = JSON.parse(text);
  delete m.key;
  delete m.update_url;
  const write = (v) => {
    if (Array.isArray(v)) return `[${v.map(write).join(",")}]`;
    if (v && typeof v === "object") {
      return `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${write(v[k])}`).join(",")}}`;
    }
    return JSON.stringify(v);
  };
  return write(m);
}

/** Same hash as `hash_extension_dir` in the app. */
export function packageHash(dir) {
  const outer = createHash("sha256");
  for (const rel of listFiles(dir)) {
    let bytes = readFileSync(join(dir, rel));
    if (rel === "manifest.json") {
      bytes = Buffer.from(canonicalManifest(bytes.toString("utf8")));
    }
    outer.update(rel);
    outer.update(Buffer.from([0]));
    outer.update(createHash("sha256").update(bytes).digest());
  }
  return outer.digest("hex");
}

/** Protocol major the extension speaks, from src/worker/config.js. */
export function configProtocol(root = ROOT) {
  const m = /export const PROTOCOL = (\d+);/.exec(readFileSync(join(root, "src/worker/config.js"), "utf8"));
  if (!m) throw new Error("PROTOCOL not found in src/worker/config.js");
  return Number(m[1]);
}

/**
 * Throws unless tag `ext-vX.Y.Z` equals the manifest version and X is the protocol major.
 * @returns {{ version: string, protocol: number }}
 */
export function checkRelease(tag, manifest, protocol) {
  const m = /^ext-v((0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*))$/.exec(tag ?? "");
  if (!m) throw new Error(`tag ${tag} must look like ext-vX.Y.Z (plain version, no pre-release)`);
  const version = m[1];
  if (manifest.version !== version) throw new Error(`tag ${tag} does not match manifest version ${manifest.version}`);
  if ("version_name" in manifest) throw new Error("the store manifest must not set version_name");
  if (Number(m[2]) !== protocol) throw new Error(`manifest major ${m[2]} is not the protocol ${protocol} (config.js PROTOCOL)`);
  return { version, protocol };
}

/**
 * The app must list the build before the extension is published (spec 13.5, 13.6). `source` is the text of
 * known_builds.rs. A listed version with `sha256: None` accepts any hash; one with a hash must equal ours.
 */
export function checkKnownBuild(source, version, packageSha256) {
  const re = /KnownBuild\s*\{\s*version:\s*"([^"]+)",\s*sha256:\s*(None|Some\(\s*"([0-9a-fA-F]{64})"\s*\))\s*,?\s*\}/g;
  for (const m of source.matchAll(re)) {
    if (m[1] !== version) continue;
    if (m[3] && m[3].toLowerCase() !== packageSha256) throw new Error(`known_builds.rs lists ${version} with another hash (${m[3]}), the build is ${packageSha256}`);
    return;
  }
  throw new Error(`known_builds.rs does not list version ${version}: ship the app release that lists it first`);
}

/** Deterministic zip of dir (entries sorted, mtimes fixed, no extra attributes). Needs the `zip` tool. */
export function zipDir(dir, zipPath) {
  const files = listFiles(dir);
  const epoch = new Date("2020-01-01T00:00:00Z");
  for (const f of files) utimesSync(join(dir, f), epoch, epoch);
  execFileSync("zip", ["-X", "-q", "-D", resolve(zipPath), "-@"], { cwd: dir, input: `${files.join("\n")}\n` });
}

/**
 * @param {{ tag: string, out: string, dist?: string, root?: string, skipKnownBuilds?: boolean }} opts
 */
export function release({ tag, out, dist, root = ROOT, skipKnownBuilds = false }) {
  const distDir = resolve(dist ?? join(root, "dist/store"));
  if (!statSync(distDir, { throwIfNoEntry: false })?.isDirectory()) throw new Error(`${distDir} does not exist: run npm run ext:build`);
  const manifest = JSON.parse(readFileSync(join(distDir, "manifest.json"), "utf8"));
  const { version, protocol } = checkRelease(tag, manifest, configProtocol(root));
  const source = JSON.parse(readFileSync(join(root, "manifest.json"), "utf8"));
  if (source.version !== version) throw new Error(`dist manifest ${version} differs from source manifest ${source.version}: rebuild`);
  const package_sha256 = packageHash(distDir);
  if (!skipKnownBuilds) {
    checkKnownBuild(readFileSync(join(root, KNOWN_BUILDS_RS), "utf8"), version, package_sha256);
  }
  mkdirSync(out, { recursive: true });
  const file = `zptah-browser-${version}.zip`;
  zipDir(distDir, join(out, file));
  const info = { version, protocol, file, sha256: sha256(readFileSync(join(out, file))), package_sha256 };
  writeFileSync(join(out, "extension-release.json"), `${JSON.stringify(info, null, 2)}\n`);
  return info;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const get = (f) => (args.includes(f) ? args[args.indexOf(f) + 1] : undefined);
  try {
    const info = release({ tag: get("--tag") ?? "", out: resolve(get("--out") ?? "release-out"), dist: get("--dist") });
    console.log(JSON.stringify(info, null, 2));
  } catch (err) {
    console.error(err instanceof Error ? err.message : err);
    process.exit(1);
  }
}
