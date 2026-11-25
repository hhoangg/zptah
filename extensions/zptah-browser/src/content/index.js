// Content script stub. The build wraps all files of this directory into one IIFE, so these
// files are plain scripts without import or export. Injected on demand via chrome.scripting.
const marker = /** @type {{ zptahContentLoaded?: boolean }} */ (globalThis);
if (!marker.zptahContentLoaded) {
  marker.zptahContentLoaded = true;
}
