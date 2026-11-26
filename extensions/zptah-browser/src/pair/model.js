// What the pairing page shows for each state of the worker's link (spec 12.3). Pure, so it is
// tested; pair.js only polls the worker and renders this.

export const POLL_MS = 500;
/** How long the page waits for the desktop before it says so (the worker's own handshake limit is 10 s). */
export const TIMEOUT_MS = 20000;
export const CLOSE_SECONDS = 5;

export const FAILED_STATES = new Set(["failed", "timeout", "wrong_build", "no_code"]);

/**
 * @typedef {"waiting" | "verifying" | "connected" | "failed" | "timeout" | "wrong_build" | "no_code"} PairState
 * @typedef {{ state: PairState, tone: "busy" | "ok" | "bad" | "warn", step: number, title: string, body: string, reason: string, retry: boolean }} PairView
 */

export const STEP_LABELS = ["Reach zptah", "Verify", "Connected"];

const NEW_CODE = "Open zptah > Settings > Browser > Connect extension to get a new pairing page.";

/** @param {string} detail */
function reasonFor(detail) {
  const d = detail.trim();
  if (d === "bad nonce") return "The pairing code in the address was not valid.";
  if (/^E_[A-Z_]+$/.test(d)) return `zptah refused the request (${d}). The pairing code may have been used or expired.`;
  if (d) return `Details: ${d}`;
  return "zptah did not accept this browser.";
}

/**
 * The protocol carries no desktop name, only the app's version, so that is what identifies it.
 * @param {{ app_version?: string } | null | undefined} s
 */
export function desktopName(s) {
  const v = typeof s?.app_version === "string" ? s.app_version.trim() : "";
  return v ? `zptah ${v} on this computer` : "zptah on this computer";
}

/**
 * @param {{ status?: string, detail?: string, app_version?: string } | null | undefined} s the worker's answer to zptah.status
 * @param {number} elapsedMs since pairing started
 * @returns {PairView}
 */
export function pairView(s, elapsedMs) {
  const status = s?.status ?? "idle";
  const detail = typeof s?.detail === "string" ? s.detail : "";
  if (status === "ready") {
    return { state: "connected", tone: "ok", step: 3, title: "Connected", body: `Paired with ${desktopName(s)}.`, reason: "", retry: false };
  }
  // Exactly what the link reports for a version mismatch: the status, or the hello check's own text.
  if (status === "protocol_mismatch" || detail === "unsupported protocol") {
    return {
      state: "wrong_build", tone: "warn", step: 1, title: "This build does not match",
      body: "zptah and this extension speak different versions, or zptah did not recognise this build.",
      reason: `Update zptah and the zptah Browser extension to the latest versions, then open a new pairing page.${detail && detail !== "unsupported protocol" ? ` (${detail})` : ""}`, retry: false,
    };
  }
  if (status === "host_missing" || (status === "pair_failed" && /not found|forbidden|not allowed/i.test(detail))) {
    return {
      state: "failed", tone: "bad", step: 1, title: "Could not reach zptah",
      body: "The zptah connector is not installed for this browser.",
      reason: "Open the zptah app once, then try again. Open zptah > Settings > Browser > Connect extension.", retry: true,
    };
  }
  if (status === "pair_failed" || status === "auth_failed" || status === "not_paired") {
    if (/timed out/i.test(detail)) return timeoutView();
    // A refusal by the app (E_*) or a bad code will not change by sending the same code again.
    const refused = /^E_[A-Z_]+$/.test(detail) || detail === "bad nonce";
    return { state: "failed", tone: "bad", step: 2, title: "Pairing failed", body: reasonFor(detail), reason: NEW_CODE, retry: !refused };
  }
  // After the pairing request the link is "pairing" or "connecting". Idle or disconnected means
  // the connection to the app dropped: say so with the reason instead of waiting for the limit.
  if (status === "disconnected" || status === "idle") {
    return {
      state: "failed", tone: "bad", step: 1, title: "The connection to zptah dropped",
      body: detail ? `Details: ${detail}` : "zptah closed the connection before pairing finished.",
      reason: "Make sure zptah is running, then try again.", retry: true,
    };
  }
  if (elapsedMs >= TIMEOUT_MS) return timeoutView();
  if (status === "connecting") {
    return { state: "verifying", tone: "busy", step: 2, title: "Verifying", body: "zptah accepted the pairing. Checking the secure connection.", reason: "", retry: false };
  }
  return { state: "waiting", tone: "busy", step: 1, title: "Waiting for zptah", body: "Looking for the zptah app on this computer. Keep it open.", reason: "", retry: false };
}

/** @returns {PairView} */
function timeoutView() {
  return {
    state: "timeout", tone: "warn", step: 1, title: "zptah did not answer",
    body: "Nothing came back from the desktop app in time.",
    reason: "Make sure zptah is running, then try again. If it keeps happening, get a new pairing page from zptah.", retry: true,
  };
}

/** @returns {PairView} */
export function noCodeView() {
  return {
    state: "no_code", tone: "bad", step: 1, title: "Open this from zptah",
    body: "This page needs the pairing code that zptah adds to its address.", reason: NEW_CODE, retry: false,
  };
}

/** @param {number} seconds */
export const countdownText = (seconds) => `Closing this tab in ${seconds} ${seconds === 1 ? "second" : "seconds"}.`;
