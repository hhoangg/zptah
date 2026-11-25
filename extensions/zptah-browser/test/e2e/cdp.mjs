// A minimal Chrome DevTools Protocol client over the browser websocket (Node's global WebSocket).
import { readFileSync } from "node:fs";
import { join } from "node:path";

/**
 * Wait for `DevToolsActivePort` in the user data dir and connect to the browser endpoint.
 * @param {string} userDataDir
 * @param {number} [timeoutMs]
 */
export async function connectBrowser(userDataDir, timeoutMs = 30000) {
  const deadline = Date.now() + timeoutMs;
  let port = "";
  while (Date.now() < deadline) {
    try {
      port = readFileSync(join(userDataDir, "DevToolsActivePort"), "utf8").split("\n")[0].trim();
      if (port) break;
    } catch {
      // not written yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  if (!port) throw new Error("the browser did not publish DevToolsActivePort");
  const version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
  return new Cdp(version.webSocketDebuggerUrl);
}

export class Cdp {
  /** @param {string} url */
  constructor(url) {
    this.ws = new WebSocket(url);
    this.nextId = 1;
    /** @type {((m: any) => void)[]} */
    this.listeners = [];
    /** @type {Map<number, { ok: (v: any) => void, fail: (e: Error) => void }>} */
    this.pending = new Map();
    this.opened = new Promise((ok, fail) => {
      this.ws.addEventListener("open", () => ok(undefined));
      this.ws.addEventListener("error", () => fail(new Error("cdp websocket error")));
    });
    this.ws.addEventListener("message", (e) => {
      const msg = JSON.parse(String(e.data));
      if (msg.method) for (const fn of this.listeners) fn(msg);
      const p = msg.id ? this.pending.get(msg.id) : undefined;
      if (!p) return;
      this.pending.delete(msg.id);
      if (msg.error) p.fail(new Error(`${msg.error.message}`));
      else p.ok(msg.result);
    });
  }

  /**
   * @param {string} method
   * @param {Record<string, unknown>} [params]
   * @param {string} [sessionId]
   */
  async send(method, params = {}, sessionId) {
    await this.opened;
    const id = this.nextId++;
    const done = new Promise((ok, fail) => this.pending.set(id, { ok, fail }));
    this.ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    return /** @type {Promise<any>} */ (done);
  }

  /**
   * Record the console and exceptions of a target with timestamps, until the browser closes.
   * @param {string} targetId
   * @param {string[]} sink
   */
  async traceConsole(targetId, sink) {
    const { sessionId } = await this.send("Target.attachToTarget", { targetId, flatten: true });
    this.listeners.push((m) => {
      if (m.sessionId !== sessionId) return;
      const stamp = new Date().toISOString().slice(11, 23);
      if (m.method === "Runtime.consoleAPICalled") {
        sink.push(`${stamp} console.${m.params.type} ${m.params.args.map((/** @type {any} */ a) => a.value ?? a.description ?? "").join(" ")}`);
      } else if (m.method === "Runtime.exceptionThrown") {
        sink.push(`${stamp} exception ${m.params.exceptionDetails.text} ${m.params.exceptionDetails.exception?.description ?? ""}`);
      }
    });
    await this.send("Runtime.enable", {}, sessionId);
  }

  async targets() {
    return (await this.send("Target.getTargets")).targetInfos;
  }

  /** @param {string} url */
  async openTab(url) {
    return (await this.send("Target.createTarget", { url })).targetId;
  }

  /**
   * Evaluate in a target (flattened session) and return the value.
   * @param {string} targetId
   * @param {string} expression
   */
  async evaluate(targetId, expression) {
    const { sessionId } = await this.send("Target.attachToTarget", { targetId, flatten: true });
    try {
      const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, sessionId);
      if (r.exceptionDetails) throw new Error(`evaluate failed: ${r.exceptionDetails.text} ${r.exceptionDetails.exception?.description ?? ""}`);
      return r.result.value;
    } finally {
      await this.send("Target.detachFromTarget", { sessionId }).catch(() => {});
    }
  }

  /**
   * The extension worker target, waiting for it to start.
   * @param {string} [extensionId]
   * @param {number} [timeoutMs]
   */
  async worker(extensionId, timeoutMs = 30000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const found = (await this.targets()).find(
        (/** @type {any} */ t) => t.type === "service_worker" && t.url.startsWith(`chrome-extension://${extensionId ?? ""}`) && t.url.endsWith("/src/worker/main.js"),
      );
      if (found) return found;
      await new Promise((r) => setTimeout(r, 300));
    }
    throw new Error("the extension worker did not start");
  }

  async close() {
    await this.send("Browser.close").catch(() => {});
    this.ws.close();
  }
}
