// Pinned Chrome for Testing: download once, verify the SHA-256, unpack into a cache. A test-only
// download (spec 16.3), never done at app runtime.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, existsSync, mkdirSync, readFileSync, realpathSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const PIN = JSON.parse(readFileSync(join(HERE, "cft-pin.json"), "utf8"));

/** @returns {string} */
export function platformKey() {
  const key = `${process.platform === "darwin" ? "mac" : process.platform}-${process.arch}`;
  if (key === "mac-arm64") return "mac-arm64";
  if (process.platform === "linux" && process.arch === "x64") return "linux64";
  throw new Error(`no pinned Chrome for Testing for ${key}`);
}

/** @param {string} file */
export function sha256File(file) {
  return new Promise((ok, fail) => {
    const hash = createHash("sha256");
    createReadStream(file).on("data", (d) => hash.update(d)).on("error", fail).on("end", () => ok(hash.digest("hex")));
  });
}

/**
 * @param {string} cacheDir
 * @returns {Promise<string>} the real path of the browser executable
 */
export async function ensureChrome(cacheDir) {
  const spec = /** @type {Record<string, string>} */ (PIN.platforms[platformKey()]);
  const root = join(cacheDir, "cft", PIN.version);
  const zip = join(root, `${spec.dir}.zip`);
  const unpacked = join(root, spec.dir);
  const exe = join(unpacked, spec.exe);
  mkdirSync(root, { recursive: true });
  if (!existsSync(zip) || (await sha256File(zip)) !== spec.sha256) {
    rmSync(zip, { force: true });
    console.log(`downloading Chrome for Testing ${PIN.version}`);
    const res = await fetch(spec.url);
    if (!res.ok) throw new Error(`download failed: HTTP ${res.status}`);
    const tmp = `${zip}.part`;
    writeFileSync(tmp, Buffer.from(await res.arrayBuffer()));
    const got = await sha256File(tmp);
    if (got !== spec.sha256) {
      rmSync(tmp, { force: true });
      throw new Error(`Chrome for Testing sha256 mismatch: got ${got}, pinned ${spec.sha256}`);
    }
    renameSync(tmp, zip);
  }
  // Always unpack the verified zip: a cached unpacked tree could not be trusted.
  rmSync(unpacked, { recursive: true, force: true });
  execFileSync("unzip", ["-q", zip, "-d", root], { stdio: "inherit" });
  if (!existsSync(exe)) throw new Error(`browser executable missing after unpack: ${resolve(exe)}`);
  return realpathSync(exe);
}
