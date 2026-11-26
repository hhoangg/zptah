// What the options page says about the connection (spec 11.3). Pure, so it is tested.

const ADVICE = "Open zptah > Settings > Browser > Connect extension.";

/**
 * @param {{ status: string, paired: boolean, stopped?: boolean, protocol: number, app_version?: string, browser?: { brand: string, version: string }, instance_id?: string }} s
 * @returns {{ text: string, advice: string, connected: boolean, tone: "ok" | "warn" | "bad" | "idle", label: string }}
 */
export function describeStatus(s) {
  const profile = `${s.browser?.brand ?? "Chrome"}, id ${String(s.instance_id ?? "").slice(0, 8)}`;
  if (s.status === "ready") {
    const app = s.app_version ? ` (${s.app_version})` : "";
    return {
      text: `Connected to zptah${app}. Paired as ${profile}. Protocol ${s.protocol}.${s.stopped ? " Stopped: resume in zptah." : ""}`,
      advice: "",
      connected: true,
      tone: "ok",
      label: s.stopped ? "Connected, stopped" : "Connected",
    };
  }
  if (!s.paired) return { text: "Not paired with zptah.", advice: ADVICE, connected: false, tone: "idle", label: "Not paired" };
  switch (s.status) {
    case "protocol_mismatch":
      return { text: "zptah and this extension speak different versions.", advice: "Update zptah and this extension, then try again.", connected: false, tone: "warn", label: "Version mismatch" };
    case "auth_failed":
    case "pair_failed":
      return { text: "zptah did not accept this browser.", advice: ADVICE, connected: false, tone: "bad", label: "Not accepted" };
    case "host_missing":
      return { text: "The zptah connector is not installed for this browser.", advice: ADVICE, connected: false, tone: "bad", label: "Connector missing" };
    default:
      return { text: "zptah is not running or not connected.", advice: "Open the zptah app. This page reconnects by itself.", connected: false, tone: "warn", label: "Offline" };
  }
}

/** @param {string[]} list @param {string} site */
export const withSite = (list, site) => (list.includes(site) ? list : [...list, site]);

/** @param {string[]} list @param {string} site */
export const withoutSite = (list, site) => list.filter((s) => s !== site);

/**
 * The connection facts shown in the card. The protocol carries no desktop name, only the app
 * version, so that identifies the desktop.
 * @param {{ status: string, paired: boolean, app_version?: string, browser?: { brand: string, version: string }, instance_id?: string, protocol: number }} s
 * @returns {[string, string][]}
 */
export function connectionFacts(s) {
  /** @type {[string, string][]} */
  const rows = [];
  if (s.status === "ready") rows.push(["Desktop", s.app_version ? `zptah ${s.app_version}` : "zptah"]);
  if (s.paired) {
    rows.push(["Browser", `${s.browser?.brand ?? "Chrome"}${s.browser?.version ? ` ${s.browser.version}` : ""}`]);
    if (s.instance_id) rows.push(["Profile id", String(s.instance_id).slice(0, 8)]);
    rows.push(["Protocol", String(s.protocol)]);
  }
  return rows;
}

/** @param {unknown} n */
export function activeMonosText(n) {
  const c = typeof n === "number" && Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
  if (c === 0) return "No Mono is working in this browser right now.";
  return c === 1 ? "1 Mono is working in this browser." : `${c} Monos are working in this browser.`;
}

// ---- the debugger fallback switch (spec 6.3, 11.3, 13.2) ----

/**
 * `debugger` is a required manifest permission because Chrome does not allow it as an optional one
 * (it is dropped and permissions.request rejects). The user's choice is the `use_debugger` pref.
 * @param {{ available: boolean, enabled: boolean }} s `available`: chrome.debugger exists
 * @returns {{ label: string, tone: "ok" | "idle" | "bad", note: string, canToggle: boolean }}
 */
export function debuggerView(s) {
  if (!s.available) {
    return {
      label: "Unavailable",
      tone: "bad",
      note: "Chrome did not give this extension the debugger permission. Reinstall or update the extension.",
      canToggle: false,
    };
  }
  return s.enabled
    ? { label: "On", tone: "ok", note: "", canToggle: true }
    : { label: "Off", tone: "idle", note: "", canToggle: true };
}
