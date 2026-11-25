// Service worker entry: owns the link to the zptah app (see link.js). Tools and the Mono
// dispatcher arrive in later tickets and register on `link.handlers`.
import { ALARM_NAME, HOST_NAME, PING_INTERVAL_MS } from "./config.js";
import { BrowserLink } from "./link.js";
import { registerReadPath } from "./readpath.js";
import { createStore, trustedArea } from "./store.js";

/** Brand and version as the browser reports them; Chrome only exposes the major here. */
function browserInfo() {
  const brands = /** @type {{ brand: string, version: string }[]} */ (
    /** @type {any} */ (navigator).userAgentData?.brands ?? []
  );
  const real = brands.find((b) => /chrome|chromium|edge|brave/i.test(b.brand) && !/not.?a.?brand/i.test(b.brand));
  if (real) return { brand: real.brand.slice(0, 64), version: real.version.slice(0, 64) };
  const m = /Chrome\/([\d.]+)/.exec(navigator.userAgent);
  return { brand: "Chrome", version: m ? m[1] : "0" };
}

// First thing: keep K out of content scripts. Every store read and write waits for this.
const storageArea = trustedArea(chrome.storage.local);

const manifest = chrome.runtime.getManifest();

export const link = new BrowserLink({
  hostName: HOST_NAME,
  connectNative: (name) => chrome.runtime.connectNative(name),
  store: createStore(storageArea),
  extVersion: (manifest.version_name ?? manifest.version).slice(0, 32),
  browser: browserInfo(),
});

registerReadPath(link);

// Messages from our own pair page: the nonce to pair with, and a status poll. Anything else
// (content scripts, other extensions) is ignored.
chrome.runtime.onMessage.addListener(
  /**
   * @param {unknown} message
   * @param {chrome.runtime.MessageSender} sender
   * @param {(response?: unknown) => void} sendResponse
   */
    (message, sender, sendResponse) => {
    const m = /** @type {{ type?: unknown, nonce?: unknown } | null} */ (message);
    const pairUrl = chrome.runtime.getURL("src/pair/pair.html");
    if (sender.id !== chrome.runtime.id || !sender.url?.startsWith(pairUrl)) return false;
    if (m?.type === "zptah.status") {
      sendResponse({ status: link.status });
      return false;
    }
    if (m?.type !== "zptah.pair" || typeof m.nonce !== "string") return false;
    link.pair(m.nonce).then(
      () => sendResponse({ ok: true }),
      () => sendResponse({ ok: false }),
  );
  return true;
  },
);

// A worker that only lives while events arrive would be stopped after 30 s idle. The storage
// read every 20 s counts as extension activity and the tick reconnects when the backoff
// allows; the alarm wakes the worker again if it was stopped anyway. Chrome 120+ allows 30 s.
function keepAlive() {
  void storageArea.get("zptah.keepalive");
  void link.tick();
}
setInterval(keepAlive, PING_INTERVAL_MS);
chrome.alarms.create(ALARM_NAME, { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(/** @param {chrome.alarms.Alarm} alarm */ (alarm) => {
  if (alarm.name === ALARM_NAME) keepAlive();
});
chrome.runtime.onStartup.addListener(() => void link.start());
chrome.runtime.onInstalled.addListener(() => void link.start());
void link.start();
