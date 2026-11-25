// One-time pairing page (spec 12.3): the app opens it with #n=<nonce>. It hands the nonce to the
// worker and closes once the worker is paired.
const msg = document.getElementById("msg");
const nonce = new URLSearchParams(location.hash.slice(1)).get("n");

/** @param {string} text */
const say = (text) => {
  if (msg) msg.textContent = text;
};

async function close() {
  const tab = await chrome.tabs.getCurrent();
  if (tab?.id !== undefined) await chrome.tabs.remove(tab.id);
  else window.close();
}

async function run() {
  if (!nonce) return say("This page must be opened from zptah.");
  // Drop the nonce from the address bar and history as soon as it is read.
  history.replaceState(null, "", location.pathname);
  const reply = await chrome.runtime.sendMessage({ type: "zptah.pair", nonce });
  if (!reply?.ok) return say("Could not start pairing. Try again from zptah.");
  for (let i = 0; i < 40; i += 1) {
    await new Promise((r) => setTimeout(r, 500));
    const s = await chrome.runtime.sendMessage({ type: "zptah.status" });
    if (s?.status === "ready") return close();
    if (s?.status === "pair_failed") return say("Pairing failed. Try again from zptah.");
  }
  say("Pairing timed out. Try again from zptah.");
}
void run();
