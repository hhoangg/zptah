// The options page (spec 11.3): connection, cursor switch and off list, debugger permission,
// version. Nothing else, and no way to talk to a Mono from here.
import { normalizeSite, PREFS_KEY, loadPrefs } from "../worker/prefs.js";
import { describeStatus, withoutSite, withSite } from "./model.js";

/** @param {string} id */
const $ = (id) => /** @type {HTMLElement} */ (document.getElementById(id));
const area = chrome.storage.local;
const DEBUGGER = { permissions: ["debugger"] };

async function renderStatus() {
  /** @type {any} */
  let s = null;
  try {
    s = await chrome.runtime.sendMessage({ type: "zptah.status" });
  } catch {
    s = null;
  }
  const d = s ? describeStatus(s) : { text: "The extension is starting.", advice: "", connected: false };
  $("status").textContent = d.text;
  $("advice").textContent = d.advice;
  $("unpair").hidden = !s?.paired;
  const m = chrome.runtime.getManifest();
  $("version").textContent = `Version ${m.version_name ?? m.version}`;
  $("ext-id").textContent = `Extension id ${chrome.runtime.id}`;
}

async function renderPrefs() {
  const prefs = await loadPrefs(area);
  /** @type {HTMLInputElement} */ ($("cursor")).checked = prefs.cursor;
  const ul = $("sites");
  ul.replaceChildren();
  for (const site of prefs.off_sites) {
    const li = document.createElement("li");
    li.append(site);
    const b = document.createElement("button");
    b.type = "button";
    b.textContent = "Remove";
    b.addEventListener("click", async () => {
      const cur = await loadPrefs(area);
      await area.set({ [PREFS_KEY]: { ...cur, off_sites: withoutSite(cur.off_sites, site) } });
      void renderPrefs();
    });
    li.append(b);
    ul.append(li);
  }
}

async function renderDebugger() {
  const has = await chrome.permissions.contains(DEBUGGER);
  /** @type {HTMLButtonElement} */ ($("debugger")).disabled = has;
  $("debugger-state").textContent = has ? "Allowed." : "Not allowed.";
}

$("cursor").addEventListener("change", async (e) => {
  const cur = await loadPrefs(area);
  await area.set({ [PREFS_KEY]: { ...cur, cursor: /** @type {HTMLInputElement} */ (e.target).checked } });
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
  const cur = await loadPrefs(area);
  await area.set({ [PREFS_KEY]: { ...cur, off_sites: withSite(cur.off_sites, site) } });
  input.value = "";
  void renderPrefs();
});

// chrome.permissions.request needs a user gesture on an extension page, which is why the button is here.
$("debugger").addEventListener("click", async () => {
  try {
    await chrome.permissions.request(DEBUGGER);
  } finally {
    void renderDebugger();
  }
});

$("unpair").addEventListener("click", async () => {
  await chrome.runtime.sendMessage({ type: "zptah.unpair" });
  void renderStatus();
});

chrome.permissions.onAdded?.addListener(() => void renderDebugger());
chrome.permissions.onRemoved?.addListener(() => void renderDebugger());
void renderStatus();
void renderPrefs();
void renderDebugger();
setInterval(() => void renderStatus(), 3000);
