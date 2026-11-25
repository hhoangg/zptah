// The toolbar badge (spec 11.2) and tooltip (11.1).

export const GREEN = "#16a34a";
export const RED = "#b91c1c";
export const AMBER = "#d97706";

const STATES_NEEDING_ATTENTION = new Set(["protocol_mismatch", "auth_failed", "not_paired", "pair_failed"]);

/**
 * @param {{ status: string, paired: boolean, stopped: boolean, active: number }} s
 * @returns {{ text: string, color: string }}
 */
export function badgeFor(s) {
  // Stop is enforced in the extension, so it shows whether or not the app is connected.
  if (s.stopped) return { text: "■", color: RED };
  if (STATES_NEEDING_ATTENTION.has(s.status) || (!s.paired && s.status === "idle")) return { text: "!", color: RED };
  if (s.status !== "ready") return { text: "…", color: AMBER };
  return { text: s.active > 0 ? String(s.active) : "", color: GREEN };
}

/** @param {boolean} stopped @param {string} lang */
export function tooltip(stopped, lang) {
  if (lang === "vi") return stopped ? "Đã dừng. Tiếp tục trong zptah." : "Dừng tất cả Mono";
  return stopped ? "Stopped. Resume in zptah." : "Stop all Monos";
}

/**
 * @param {object} deps
 * @param {any} deps.api chrome
 * @param {{ status: string, paired: boolean, stopped: boolean }} deps.link
 * @param {() => number} deps.active Monos with a run in progress or an action in the last 60 s
 * @param {string} [deps.lang]
 */
export function createBadge(deps) {
  /** @type {string} */
  let last = "";
  /** Writes the badge when it changed. */
  function update() {
    const b = badgeFor({ status: deps.link.status, paired: deps.link.paired, stopped: deps.link.stopped === true, active: deps.active() });
    const title = tooltip(deps.link.stopped === true, deps.lang ?? "en");
    const key = `${b.text}|${b.color}|${title}`;
    if (key === last) return b;
    last = key;
    try {
      void deps.api.action?.setBadgeText?.({ text: b.text });
      void deps.api.action?.setBadgeBackgroundColor?.({ color: b.color });
      void deps.api.action?.setTitle?.({ title });
    } catch {
      // The action API is not available (a test double).
    }
    return b;
  }
  return { update };
}
