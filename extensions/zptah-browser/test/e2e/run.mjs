// Headless end-to-end test against a pinned Chrome for Testing (spec 16.3). Plays the browser
// side (real extension, real native messaging host) while the Rust example `browser_e2e_app`
// plays the app side and runs the real link server and read tools.
//
// Safety: headless only (`--headless=new`), a throwaway `--user-data-dir` under a fresh temp
// directory, the host manifest inside that directory, and the browser is killed at the end. It
// never uses an installed browser or its profile.
import { spawn, spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { createInterface } from "node:readline";
import { openSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { platform, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "../../scripts/build.mjs";
import { ensureChrome } from "./cft.mjs";
import { connectBrowser } from "./cdp.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const EXT_ROOT = resolve(HERE, "../..");
const REPO = resolve(EXT_ROOT, "../..");
const DEV_ID = "bmmfejkhajglhijajgdgfckhonddijjh";
const DEV_HOST = "app.zptah.browser_host_dev";
const CACHE = process.env.ZPTAH_E2E_CACHE ?? join(REPO, ".cache", "zptah-e2e");

const PAGES = {
  "/": `<!doctype html><title>Fixture shop</title><h1>Fixture shop</h1><p>Hello from the fixture page.</p>
<button id="buy">Buy now</button> <a href="/article">Read the article</a>`,
  "/article": `<!doctype html><title>Quiet harbors</title><article><h1>Quiet harbors</h1>
<p>The harbor was quiet at dawn, and the boats rocked slowly against the old stone pier.</p>
<p>Fishermen mended their nets while gulls circled the empty market square above them.</p></article>`,
};

/** @type {{ name: string, ok: boolean, detail?: string }[]} */
const results = [];
/** What to print when a check failed: the temp dir (browser logs) and the app side. @type {{ base?: string, app?: { stderr: string }, worker: string[] }} */
const diag = { worker: /** @type {string[]} */ ([]) };
/**
 * @param {string} name
 * @param {boolean} ok
 * @param {string} [detail]
 */
function check(name, ok, detail) {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${ok || !detail ? "" : `\n     ${detail}`}`);
}

const cleanups = /** @type {(() => Promise<void> | void)[]} */ ([]);
async function cleanup() {
  while (cleanups.length) {
    try {
      await cleanups.pop()?.();
    } catch (err) {
      console.error(`cleanup: ${err}`);
    }
  }
}
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => void cleanup().finally(() => process.exit(130)));
}

/** @param {string} dir */
function removeTemp(dir) {
  // Only ever remove a directory this run created: `zpe-*` directly under a temp root.
  const roots = [realpathSync("/tmp"), realpathSync(tmpdir())];
  if (basename(dir).startsWith("zpe-") && roots.includes(dirname(dir))) rmSync(dir, { recursive: true, force: true });
}

/**
 * Launch headless Chrome for Testing with an isolated profile.
 * @param {{ exe: string, profile: string, extension: string, env?: Record<string, string> }} o
 */
function launchChrome(o) {
  const args = [
    "--headless=new",
    `--user-data-dir=${o.profile}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--remote-debugging-port=0",
    `--load-extension=${o.extension}`,
    "--disable-features=DisableLoadExtensionCommandLineSwitch",
    ...(platform() === "linux" ? ["--no-sandbox", "--disable-dev-shm-usage"] : []),
    "about:blank",
  ];
  const log = openSync(join(o.profile, "..", `browser-${basename(o.profile)}.log`), "a");
  const child = spawn(o.exe, ["--enable-logging=stderr", "--v=1", ...args], { stdio: ["ignore", "ignore", log], env: { ...process.env, ...o.env } });
  cleanups.push(() => stop(child));
  return child;
}

/** @param {import("node:child_process").ChildProcess} child */
async function stop(child) {
  if (child.exitCode !== null || child.signalCode) return;
  child.kill("SIGTERM");
  await new Promise((r) => {
    const t = setTimeout(() => {
      child.kill("SIGKILL");
      r(undefined);
    }, 8000);
    child.once("exit", () => {
      clearTimeout(t);
      r(undefined);
    });
  });
}

/** The line-delimited JSON channel to `browser_e2e_app`. */
class App {
  /** @param {import("node:child_process").ChildProcessWithoutNullStreams} child */
  constructor(child) {
    this.child = child;
    this.nextId = 1;
    /** @type {Map<number, (v: any) => void>} */
    this.pending = new Map();
    this.ready = new Promise((ok) => (this.onReady = ok));
    this.stderr = "";
    child.stderr.on("data", (d) => (this.stderr += d));
    createInterface({ input: child.stdout }).on("line", (line) => {
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        return;
      }
      if (msg.event === "ready") this.onReady(msg);
      else this.pending.get(msg.id)?.(msg);
    });
  }

  /**
   * @param {Record<string, unknown>} cmd
   * @param {number} [timeoutMs]
   */
  call(cmd, timeoutMs = 120000) {
    const id = this.nextId++;
    return new Promise((ok, fail) => {
      const t = setTimeout(() => fail(new Error(`app command ${cmd.cmd} timed out\n${this.stderr}`)), timeoutMs);
      this.pending.set(id, (v) => {
        clearTimeout(t);
        ok(v);
      });
      this.child.stdin.write(`${JSON.stringify({ ...cmd, id })}\n`);
    });
  }

  /**
   * Run one tool and return its text. Throws when the tool reports an error.
   * @param {string} name
   * @param {Record<string, unknown>} [args]
   */
  async tool(name, args = {}) {
    const r = /** @type {any} */ (await this.call({ cmd: "tool", name, args }));
    if (!r.ok) throw new Error(`${name}: ${r.error}`);
    const text = (r.result?.content ?? []).map((/** @type {any} */ c) => c.text ?? "").join("\n");
    return { text, isError: Boolean(r.result?.isError) };
  }
}

/** @param {number} ms */
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function main() {
  console.log("building the extension");
  build();
  const devDir = join(EXT_ROOT, "dist/dev");
  const storeDir = join(EXT_ROOT, "dist/store");

  let appExe = process.env.ZPTAH_E2E_APP;
  if (!appExe) {
    console.log("building the app side (cargo, native-dev profile)");
    const r = spawnSync(
      "cargo",
      ["build", "--locked", "--manifest-path", join(REPO, "crates/monocode-gpui/Cargo.toml"), "--profile", "native-dev",
        "-p", "monocode-core", "--example", "browser_e2e_app", "--features", "monocode-core/test-support"],
      { stdio: "inherit", cwd: REPO },
    );
    if (r.status !== 0) throw new Error("cargo build failed");
    appExe = join(REPO, "crates/monocode-gpui/target/native-dev/examples/browser_e2e_app");
  }
  appExe = realpathSync(appExe);

  const chrome = await ensureChrome(CACHE);

  // A short path: the unix socket path must fit sun_path.
  const base = realpathSync(mkdtempSync(join(platform() === "darwin" ? "/tmp" : tmpdir(), "zpe-")));
  diag.base = base;
  cleanups.push(() => removeTemp(base));
  const profile = join(base, "p");
  const socketDir = join(base, "s");
  mkdirSync(join(profile, "NativeMessagingHosts"), { recursive: true });
  // F10: Chrome for Testing finds host manifests in <user-data-dir>/NativeMessagingHosts.
  writeFileSync(
    join(profile, "NativeMessagingHosts", `${DEV_HOST}.json`),
    `${JSON.stringify({ name: DEV_HOST, description: "zptah Browser host (e2e)", path: appExe, type: "stdio", allowed_origins: [`chrome-extension://${DEV_ID}/`] }, null, 2)}\n`,
  );

  // Fixture pages.
  const server = createServer((req, res) => {
    const body = /** @type {Record<string, string>} */ (PAGES)[(req.url ?? "/").split("?")[0]];
    res.writeHead(body ? 200 : 404, { "content-type": "text/html; charset=utf-8" });
    res.end(body ?? "not found");
  });
  await new Promise((ok) => server.listen(0, "127.0.0.1", () => ok(undefined)));
  cleanups.push(() => void server.close());
  const fixture = `http://127.0.0.1:${/** @type {any} */ (server.address()).port}`;

  // The app side.
  const appChild = spawn(appExe, ["--socket-dir", socketDir, "--browser-exe", chrome], { stdio: ["pipe", "pipe", "pipe"] });
  const app = new App(appChild);
  diag.app = app;
  cleanups.push(async () => {
    await app.call({ cmd: "quit" }, 5000).catch(() => {});
    await stop(appChild);
  });
  await Promise.race([app.ready, sleep(60000).then(() => Promise.reject(new Error(`the app did not start\n${app.stderr}`)))]);

  // The browser, with the dev extension.
  const env = { ZPTAH_DEV_SOCKET_DIR: socketDir, ZPTAH_DEV_EXTRA_BROWSER: chrome };
  const browser = launchChrome({ exe: chrome, profile, extension: devDir, env });
  const cdp = await connectBrowser(profile);
  const worker = await cdp.worker(DEV_ID);
  await cdp.traceConsole(worker.targetId, diag.worker);
  check("the dev extension loads with the pinned id (F9)", worker.url.includes(DEV_ID));

  // Pairing.
  const pair = /** @type {any} */ (await app.call({ cmd: "pair" }));
  check("the app opens a pairing window", pair.ok && pair.url.startsWith(`chrome-extension://${DEV_ID}/src/pair/pair.html#n=`), JSON.stringify(pair));
  await cdp.openTab(pair.url);
  const paired = /** @type {any} */ (await app.call({ cmd: "wait", kind: "paired", timeout_ms: 60000 }));
  check("pairing completes", paired.ok, JSON.stringify(paired));
  const connected = /** @type {any} */ (await app.call({ cmd: "wait", kind: "connected", timeout_ms: 60000 }));
  check("the extension connects after pairing", connected.ok, JSON.stringify(connected));
  const links = /** @type {any} */ (await app.call({ cmd: "links" }));
  check("one paired profile is connected", links.ok && links.links.length === 1 && links.links[0].connected, JSON.stringify(links));

  // A user tab, then the read tools.
  const activeTabs = async () =>
    JSON.stringify(
      /** @type {any[]} */ (await cdp.evaluate(worker.targetId, "chrome.tabs.query({active:true})"))
        .map((t) => [t.windowId, t.id])
        .sort(),
    );
  await cdp.openTab(`${fixture}/`);
  await cdp.openTab(`${fixture}/article`);
  await sleep(1500);
  const activeBefore = await activeTabs();

  const listed = await app.tool("browser_tabs", { action: "list", all: true });
  const shop = listed.text.split("\n").find((/** @type {string} */ l) => l.includes(`${fixture}/`) && !l.includes("/article"));
  const article = listed.text.split("\n").find((/** @type {string} */ l) => l.includes(`${fixture}/article`));
  check("all tabs are listed, including the user's", Boolean(shop && article) && !listed.isError, listed.text);
  const tabId = (/** @type {string | undefined} */ line) => Number(/\[tab=(\d+)\]/.exec(line ?? "")?.[1]);
  check("a user tab is marked as the user's", /user/.test(shop ?? ""), shop);

  const snap = await app.tool("browser_snapshot", { tab: tabId(shop) });
  check("snapshot shows the heading and the button", /heading "Fixture shop"/.test(snap.text) && /button "Buy now"/.test(snap.text) && !snap.isError, snap.text);
  const read = await app.tool("browser_read", { tab: tabId(article) });
  check("read returns the article text", /harbor was quiet at dawn/.test(read.text) && !read.isError, read.text);
  const found = await app.tool("browser_find", { tab: tabId(shop), query: "Buy now" });
  check("find returns the button with a ref", /\[ref=e\d+\]|\be\d+\b/.test(found.text) && /button/.test(found.text) && /Buy now/.test(found.text) && !found.isError, found.text);

  const activeAfter = await activeTabs();
  check("no tab became active during the reads", activeBefore === activeAfter, `${activeBefore} -> ${activeAfter}`);

  // Teardown of the dev browser before the store build check.
  await app.call({ cmd: "quit" }, 5000).catch(() => {});
  await cdp.close();
  await stop(browser);

  // The store build loads and carries no test hooks.
  /** @type {string[]} */
  const storeFiles = [];
  const walk = (/** @type {string} */ d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) (e.isDirectory() ? walk(join(d, e.name)) : storeFiles.push(join(d, e.name)));
  };
  walk(storeDir);
  const hooked = storeFiles.filter((f) => /\.(js|html|json)$/.test(f) && readFileSync(f, "utf8").includes("__zptah"));
  check("the store build has no test hooks on disk", hooked.length === 0, hooked.join(", "));
  const storeManifest = JSON.parse(readFileSync(join(storeDir, "manifest.json"), "utf8"));
  check("the store manifest has no key", !("key" in storeManifest));
  const storeProfile = join(base, "store-p");
  mkdirSync(storeProfile);
  const storeBrowser = launchChrome({ exe: chrome, profile: storeProfile, extension: storeDir });
  const storeCdp = await connectBrowser(storeProfile);
  const storeWorker = await storeCdp.worker();
  check("the store build loads", storeWorker.url.endsWith("/src/worker/main.js") && !storeWorker.url.includes(DEV_ID), storeWorker.url);
  const leaked = await storeCdp.evaluate(storeWorker.targetId, "Object.getOwnPropertyNames(globalThis).filter((k) => k.startsWith('__zptah'))");
  check("the store worker exposes no test hooks", Array.isArray(leaked) && leaked.length === 0, JSON.stringify(leaked));
  await storeCdp.close();
  await stop(storeBrowser);
}

/** Browser logs (extension console and native messaging lines) and the app's stderr. */
function printDiagnostics() {
  console.log("--- diagnostics ---");
  console.log(`extension worker console:\n${diag.worker.join("\n")}`);
  console.log(`app stderr:\n${diag.app?.stderr ?? "(none)"}`);
  if (!diag.base) return;
  for (const name of readdirSync(diag.base).filter((n) => n.startsWith("browser-"))) {
    const lines = readFileSync(join(diag.base, name), "utf8").split("\n").filter((l) => /zptah|native_message|NativeMessage|host/i.test(l));
    console.log(`${name} (${lines.length} matching lines, last 150):\n${lines.slice(-150).join("\n")}`);
  }
}

let failure = null;
try {
  await main();
} catch (err) {
  failure = err;
  console.error(`e2e error: ${err instanceof Error ? err.stack : err}`);
} finally {
  if (results.some((r) => !r.ok) || failure) printDiagnostics();
  await cleanup();
}
const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length} passed, ${failed.length} failed${failure ? ", run aborted" : ""}`);
process.exit(failed.length === 0 && !failure ? 0 : 1);
