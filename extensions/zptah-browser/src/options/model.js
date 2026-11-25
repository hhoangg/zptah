// What the options page says about the connection (spec 11.3). Pure, so it is tested.

const ADVICE = "Open zptah > Settings > Browser > Connect extension.";

/**
 * @param {{ status: string, paired: boolean, stopped?: boolean, protocol: number, app_version?: string, browser?: { brand: string, version: string }, instance_id?: string }} s
 * @returns {{ text: string, advice: string, connected: boolean }}
 */
export function describeStatus(s) {
  const profile = `${s.browser?.brand ?? "Chrome"}, id ${String(s.instance_id ?? "").slice(0, 8)}`;
  if (s.status === "ready") {
    const app = s.app_version ? ` (${s.app_version})` : "";
    return {
      text: `Connected to zptah${app}. Paired as ${profile}. Protocol ${s.protocol}.${s.stopped ? " Stopped: resume in zptah." : ""}`,
      advice: "",
      connected: true,
    };
  }
  if (!s.paired) return { text: "Not paired with zptah.", advice: ADVICE, connected: false };
  switch (s.status) {
    case "protocol_mismatch":
      return { text: "zptah and this extension speak different versions.", advice: "Update zptah and this extension, then try again.", connected: false };
    case "auth_failed":
    case "pair_failed":
      return { text: "zptah did not accept this browser.", advice: ADVICE, connected: false };
    case "host_missing":
      return { text: "The zptah connector is not installed for this browser.", advice: ADVICE, connected: false };
    default:
      return { text: "zptah is not running or not connected.", advice: "Open the zptah app. This page reconnects by itself.", connected: false };
  }
}

/** @param {string[]} list @param {string} site */
export const withSite = (list, site) => (list.includes(site) ? list : [...list, site]);

/** @param {string[]} list @param {string} site */
export const withoutSite = (list, site) => list.filter((s) => s !== site);
