// Service worker entry: owns the link to the zptah app (see link.js). Tools and the Mono
// dispatcher arrive in later tickets and register on `link.handlers`.
import { registerActPath } from "./act.js";
import { createBadge } from "./badge.js";
import { ALARM_NAME, HOST_NAME, PING_INTERVAL_MS, PROTOCOL } from "./config.js";
import { createDebugger } from "./debugger.js";
import { createEffects } from "./effects.js";
import { createFrontGuard } from "./front-guard.js";
import { Lexicon } from "./lexicon.js";
import { LEXICON_DATA } from "./lexicon-data.js";
import { BrowserLink } from "./link.js";
import { registerOwnership } from "./ownership.js";
import { registerReadPath } from "./readpath.js";
import { createStop, overlayStopSender } from "./stop.js";
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

/** @type {ReturnType<typeof registerOwnership> | null} */
let ownership = null;

/** @type {ReturnType<typeof createEffects> | null} */
let effects = null;
/** @type {ReturnType<typeof createBadge> | null} */
let badge = null;

export const link = new BrowserLink({
  // A connection brings its roster in `auth`: the groups take the Monos' titles and colors.
  onStatus: (status) => {
    if (status === "ready") void ownership?.syncRoster();
    badge?.update();
  },
  hostName: HOST_NAME,
  connectNative: (name) => chrome.runtime.connectNative(name),
  store: createStore(storageArea),
  extVersion: (manifest.version_name ?? manifest.version).slice(0, 32),
  browser: browserInfo(),
});

// One debugger wrapper (and CDP allowlist) for the screenshots of the read path and the act path.
const dbg = createDebugger({
  api: /** @type {any} */ (chrome),
  now: Date.now,
  sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  emit: (method, params) => void link.sendEvent(method, params),
  isStopped: () => link.stopped === true,
});
const front = createFrontGuard({ api: /** @type {any} */ (chrome), now: Date.now });
const read = registerReadPath(link, {
  dbg,
  around: (tab, fn) => (effects ? effects.around(tab, fn) : fn()),
  ownerOf: (tab) => ownership?.ownerOf(tab) ?? null,
  onRoster: () => void ownership?.syncRoster(),
  groupTitle: (mono) => ownership?.titleOfMono(mono),
});
ownership = registerOwnership(link, { read, front });
const uiLang = () => chrome.i18n?.getUILanguage?.() ?? "en";
effects = createEffects({
  api: /** @type {any} */ (chrome),
  now: Date.now,
  sleep: (ms) => new Promise((r) => setTimeout(r, ms)),
  ownerOf: (tab) => ownership?.ownerOf(tab) ?? null,
  area: storageArea,
  debuggerAttached: (tab) => dbg.isAttached(tab),
  lang: uiLang,
  onActivity: () => badge?.update(),
});
badge = createBadge({ api: /** @type {any} */ (chrome), link, active: () => effects?.activeMonos() ?? 0, lang: uiLang() });
badge.update();
const acts = registerActPath(link, {
  read,
  dbg,
  effects,
  onStopState: () => badge?.update(),
  lexicon: LEXICON_DATA ? new Lexicon(LEXICON_DATA.lexicon, LEXICON_DATA.sites) : null,
});

// The overlay's Stop (spec 10a): the tab goes back to the user and the Mono's work on it ends.
const stop = createStop({
  ownerOf: (tab) => ownership?.ownerOf(tab) ?? null,
  stopTab: (tab) => acts.stopTab(tab),
  clearEffects: (tab) => effects?.clear(tab) ?? Promise.resolve(),
  takeBack: async (tab) => {
    ownership?.takeBack(tab);
    await ownership?.groups.ungroup(tab);
  },
  emit: (method, params) => void link.sendEvent(method, params),
});

// Messages: from our own pair and options pages (nonce to pair with, a status poll, unpair) and
// the overlay's Stop from our content script. Anything else (other extensions, pages) is ignored.
chrome.runtime.onMessage.addListener(
  /**
   * @param {unknown} message
   * @param {chrome.runtime.MessageSender} sender
   * @param {(response?: unknown) => void} sendResponse
   */
  (message, sender, sendResponse) => {
    const m = /** @type {{ type?: unknown, nonce?: unknown } | null} */ (message);
    if (sender.id !== chrome.runtime.id) return false;
    if (m?.type === "zptah.overlay_stop") {
      // The content script of a page: the tab comes from the sender, never from the message.
      const tab = overlayStopSender(sender, chrome.runtime.id);
      if (tab === null) return false;
      void stop.stopTab(tab).then(
        (r) => sendResponse(r),
        () => sendResponse({ stopped: false }),
      );
      return true;
    }
    const pairUrl = chrome.runtime.getURL("src/pair/pair.html");
    const optionsUrl = chrome.runtime.getURL("src/options/options.html");
    const fromPair = sender.url?.startsWith(pairUrl) === true;
    const fromOptions = sender.url?.startsWith(optionsUrl) === true;
    if (!fromPair && !fromOptions) return false;
    if (m?.type === "zptah.status") {
      sendResponse(
        fromOptions
          ? {
              status: link.status,
              paired: link.paired,
              stopped: link.stopped === true,
              protocol: PROTOCOL,
              app_version: link.appVersion,
              ext_version: manifest.version_name ?? manifest.version,
              ext_id: chrome.runtime.id,
              instance_id: link.instanceId,
              browser: browserInfo(),
            }
          : { status: link.status },
      );
      return false;
    }
    if (fromOptions && m?.type === "zptah.unpair") {
      link.unpair().then(
        () => sendResponse({ ok: true }),
        () => sendResponse({ ok: false }),
      );
      return true;
    }
    if (!fromPair || m?.type !== "zptah.pair" || typeof m.nonce !== "string") return false;
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
  void acts.sweep();
  void effects?.sweep();
  badge?.update();
}
setInterval(keepAlive, PING_INTERVAL_MS);
chrome.alarms.create(ALARM_NAME, { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener(/** @param {chrome.alarms.Alarm} alarm */ (alarm) => {
  if (alarm.name === ALARM_NAME) keepAlive();
});
chrome.runtime.onStartup.addListener(() => void link.start());
chrome.runtime.onInstalled.addListener(() => void link.start());
void link.start();
