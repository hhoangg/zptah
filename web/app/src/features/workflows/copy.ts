// Text and formats of the read-only Workflows screens. Names, summaries, outputs and errors come from the
// desktop (and from the models and commands a workflow ran), so they are untrusted: rendered as plain React text,
// capped, and stripped of hidden characters (see `features/cloud/safe.ts`).
import { safeLabel } from "@/features/accounts/pair/untrusted";

export const WORKFLOWS_NAV_LABEL = "Workflows";

export const MAX_OUTPUT_CHARS = 2_000;

const STATUS_LABELS: Readonly<Record<string, string>> = {
  pending: "Pending",
  running: "Running",
  waitingApproval: "Waiting for approval",
  waitingForUser: "Waiting for you",
  queued: "Queued",
  done: "Done",
  doneWithFailures: "Done with failures",
  failed: "Failed",
  skipped: "Skipped",
  stopped: "Stopped",
  interrupted: "Interrupted",
};

/** "doneWithFailures" reads "Done with failures"; a status this app does not know reads as its own words. */
export function statusLabel(status: string): string {
  const known = STATUS_LABELS[status];
  if (known) return known;
  const words = status.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ");
  const text = safeLabel(words, 40, "Unknown");
  return text.charAt(0).toUpperCase() + text.slice(1).toLowerCase();
}

const CALLER_LABELS: Readonly<Record<string, string>> = { manual: "Started by hand", schedule: "Schedule", event: "Event", mono: "A Mono" };

export function callerLabel(kind: string): string {
  return CALLER_LABELS[kind] ?? safeLabel(kind, 24, "Unknown");
}

export const safeText = (text: string | null | undefined, max = 200): string => safeLabel(text ?? "", max, "");

/** "Mar 3, 14:05"; a time that is not a number reads "unknown". */
export function timeLabel(ms: number | null | undefined): string {
  if (typeof ms !== "number" || !Number.isFinite(ms)) return "unknown";
  return new Date(ms).toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** "3 min 12 s", "45 s" or "2 h 5 min". */
export function durationLabel(ms: number | null | undefined): string {
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) return "";
  const seconds = Math.round(ms / 1000);
  if (seconds < 60) return `${seconds} s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min ${seconds % 60} s`;
  return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

export function nextRunLabel(ms: number | null | undefined, enabled: boolean): string {
  if (!enabled) return "Paused";
  return typeof ms === "number" ? `Next run ${timeLabel(ms)}` : "No run scheduled";
}

/** The output of a node as short plain text: a string as it is, anything else as JSON. */
export function outputText(output: unknown): string {
  if (output === undefined || output === null) return "";
  let text: string;
  try {
    text = typeof output === "string" ? output : (JSON.stringify(output, null, 2) ?? "");
  } catch {
    return "";
  }
  // Control characters other than the line break and tab never reach the screen.
  const clean = text.replace(/[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu, (char) => (char === "\n" || char === "\t" ? char : ""));
  const chars = [...clean];
  return chars.length > MAX_OUTPUT_CHARS ? `${chars.slice(0, MAX_OUTPUT_CHARS - 1).join("")}…` : clean;
}
