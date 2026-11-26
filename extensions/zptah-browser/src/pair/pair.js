// One-time pairing page (spec 12.3): the app opens it with #n=<nonce>. It hands the nonce to the
// worker, shows what the worker's link is doing, and closes once the worker is paired.
import { CLOSE_SECONDS, countdownText, noCodeView, pairView, POLL_MS } from "./model.js";

/** @param {string} id */
const $ = (id) => /** @type {HTMLElement} */ (document.getElementById(id));
const nonce = new URLSearchParams(location.hash.slice(1)).get("n");

let runId = 0;
/** @type {ReturnType<typeof setInterval> | undefined} */
let countdownTimer;

async function close() {
  const tab = await chrome.tabs.getCurrent();
  if (tab?.id !== undefined) await chrome.tabs.remove(tab.id);
  else window.close();
}

/** @param {import("./model.js").PairView} v @param {string} [extra] */
function render(v, extra) {
  const state = $("state");
  state.className = `pair-state tone-${v.tone}`;
  const glyph = $("glyph");
  glyph.replaceChildren();
  if (v.tone === "busy") {
    const sp = document.createElement("div");
    sp.className = "spinner";
    glyph.append(sp);
  } else {
    glyph.textContent = v.tone === "ok" ? "✓" : v.tone === "bad" ? "×" : "!";
  }
  $("pair-title").textContent = v.title;
  $("body").textContent = v.body;
  const reason = $("reason");
  reason.textContent = v.reason;
  reason.hidden = v.reason === "";
  const countdown = $("countdown");
  countdown.textContent = extra ?? "";
  countdown.hidden = !extra;
  document.title = `${v.title} - zptah Browser`;
  ["1", "2", "3"].forEach((_, i) => {
    const li = $("steps").children[i];
    li.className = v.state === "connected" || i + 1 < v.step ? "done" : i + 1 === v.step && v.tone !== "bad" ? "active" : "";
  });
  $("retry").hidden = !v.retry;
  // Close is offered once the page is done: success, or a failure that Retry cannot fix.
  $("close").hidden = v.tone === "busy" || (v.retry && v.state !== "connected");
}

/** After success: count down, then close. The Close button does it at once. @param {import("./model.js").PairView} v */
function startCountdown(v) {
  let left = CLOSE_SECONDS;
  render(v, countdownText(left));
  $("close").focus();
  countdownTimer = setInterval(() => {
    left -= 1;
    if (left <= 0) {
      clearInterval(countdownTimer);
      void close();
      return;
    }
    render(v, countdownText(left));
  }, 1000);
}

/** @param {string} code */
async function run(code) {
  const id = ++runId;
  clearInterval(countdownTimer);
  const started = Date.now();
  $("retry").hidden = true;
  render(pairView({ status: "pairing" }, 0));
  /** @type {any} */
  let reply = null;
  try {
    reply = await chrome.runtime.sendMessage({ type: "zptah.pair", nonce: code });
  } catch {
    reply = null;
  }
  if (id !== runId) return;
  if (!reply?.ok) {
    // The worker refused before it connected (a bad nonce) or did not answer at all.
    const s = await chrome.runtime.sendMessage({ type: "zptah.status" }).catch(() => null);
    const v = pairView(s?.status === "pair_failed" ? s : { status: "pair_failed", detail: "" }, Date.now() - started);
    return render(v);
  }
  for (;;) {
    await new Promise((r) => setTimeout(r, POLL_MS));
    if (id !== runId) return;
    const s = await chrome.runtime.sendMessage({ type: "zptah.status" }).catch(() => null);
    const v = pairView(s, Date.now() - started);
    if (v.state === "connected") return startCountdown(v);
    render(v);
    if (v.tone !== "busy") return;
  }
}

$("close").addEventListener("click", () => {
  clearInterval(countdownTimer);
  void close();
});
$("retry").addEventListener("click", () => {
  if (nonce) void run(nonce);
});

if (!nonce) {
  render(noCodeView());
} else {
  // Drop the nonce from the address bar and history as soon as it is read (it stays in memory for Retry).
  history.replaceState(null, "", location.pathname);
  void run(nonce);
}
