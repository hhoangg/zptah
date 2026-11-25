// Options page skeleton: shows the extension version. Settings arrive in a later ticket.
const el = document.getElementById("version");
if (el) el.textContent = `Version ${chrome.runtime.getManifest().version}`;
