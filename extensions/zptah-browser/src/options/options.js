// The options page (spec 11.3): connection, active Monos, cursor switch and off list, debugger
// switch, version. No way to talk to a Mono from here.
import { createPrefsWriter, normalizeSite, loadPrefs } from "../worker/prefs.js";
import {
  activeMonosText,
  connectionFacts,
  debuggerView,
  describeStatus,
  withoutSite,
  withSite,
} from "./model.js";

/** @param {string} id */
const $ = (id) => /** @type {HTMLElement} */ (document.getElementById(id));
const area = chrome.storage.local;
const writePrefs = createPrefsWriter(area);

/** Sets text only when it changed, so a screen reader is not told the same thing every poll. @param {HTMLElement} el @param {string} text */
const setText = (el, text) => {
  if (el.textContent !== text) el.textContent = text;
};

/** Runs a settings change and shows the failure instead of dropping it. @param {(cur: import("../worker/prefs.js").Prefs) => import("../worker/prefs.js").Prefs} change */
async function save(change) {
  try {
    await writePrefs(change);
    setText($("save-error"), "");
    return true;
  } catch {
    setText($("save-error"), "Could not save this setting. Try again.");
    return false;
  }
}

async function renderStatus() {
  /** @type {any} */
  let s = null;
  try {
    s = await chrome.runtime.sendMessage({ type: "zptah.status" });
  } catch {
    s = null;
  }
  const d = s
    ? describeStatus(s)
    : { text: "The extension is starting.", advice: "", connected: false, tone: /** @type {const} */ ("warn"), label: "Starting" };
  setText($("status"), d.text);
  setText($("advice"), d.advice);
  setText($("status-label"), d.label);
  $("status-pill-inner").className = `pill tone-${d.tone}`;
  const rows = s ? connectionFacts(s) : [];
  const facts = $("facts");
  const sig = JSON.stringify(rows);
  if (facts.dataset.sig !== sig) {
    facts.dataset.sig = sig;
    facts.replaceChildren();
    for (const [k, v] of rows) {
      const dt = document.createElement("dt");
      dt.textContent = k;
      const dd = document.createElement("dd");
      dd.textContent = v;
      facts.append(dt, dd);
    }
  }
  $("unpair").hidden = !s?.paired;
  $("reconnect").hidden = !s?.paired || d.connected;
  // Active Monos come from the badge data; without it the card is left out.
  const monos = $("monos-card");
  if (s && typeof s.active_monos === "number" && d.connected) {
    monos.hidden = false;
    setText($("monos"), activeMonosText(s.active_monos));
  } else {
    monos.hidden = true;
  }
  const m = chrome.runtime.getManifest();
  $("version").textContent = m.version_name ?? m.version;
  $("ext-id").textContent = chrome.runtime.id;
}

async function renderPrefs() {
  const prefs = await loadPrefs(area);
  /** @type {HTMLInputElement} */ ($("cursor")).checked = prefs.cursor;
  showDebugger(prefs.use_debugger);
  const ul = $("sites");
  ul.replaceChildren();
  $("sites-empty").hidden = prefs.off_sites.length > 0;
  for (const site of prefs.off_sites) {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "site";
    name.textContent = site;
    li.append(name);
    const b = document.createElement("button");
    b.type = "button";
    b.className = "small";
    b.textContent = "Remove";
    b.setAttribute("aria-label", `Remove ${site}`);
    b.addEventListener("click", async () => {
      await save((cur) => ({ ...cur, off_sites: withoutSite(cur.off_sites, site) }));
      void renderPrefs();
    });
    li.append(b);
    ul.append(li);
  }
}

/** @param {boolean} enabled */
function showDebugger(enabled) {
  const v = debuggerView({ available: Boolean(chrome.debugger), enabled });
  const box = /** @type {HTMLInputElement} */ ($("debugger"));
  box.checked = enabled && v.canToggle;
  box.disabled = !v.canToggle;
  setText($("debugger-state"), `Debugger fallback ${v.label.toLowerCase()}.`);
  const note = $("debugger-note");
  setText(note, v.note);
  note.hidden = v.note === "";
}

$("cursor").addEventListener("change", async (e) => {
  const on = /** @type {HTMLInputElement} */ (e.target).checked;
  if (!(await save((cur) => ({ ...cur, cursor: on })))) void renderPrefs();
});

$("debugger").addEventListener("change", async (e) => {
  const on = /** @type {HTMLInputElement} */ (e.target).checked;
  if (!(await save((cur) => ({ ...cur, use_debugger: on })))) void renderPrefs();
});

$("site-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const input = /** @type {HTMLInputElement} */ ($("site"));
  const site = normalizeSite(input.value);
  if (!site) {
    $("site-error").textContent = "Use a host such as example.com or *.example.com.";
    return;
  }
  $("site-error").textContent = "";
  if (await save((cur) => ({ ...cur, off_sites: withSite(cur.off_sites, site) }))) input.value = "";
  void renderPrefs();
});

$("unpair").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "zptah.unpair" }).catch(() => null);
  void renderStatus();
});
$("reconnect").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "zptah.reconnect" }).catch(() => null);
  void renderStatus();
});

// Any change of the settings (this page, another options tab) redraws the list and switches.
chrome.storage.onChanged.addListener(() => void renderPrefs());
void renderStatus();
void renderPrefs();
setInterval(() => void renderStatus(), 3000);
