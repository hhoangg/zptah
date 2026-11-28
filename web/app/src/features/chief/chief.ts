// The Chief: a special Mono that proposes commands for coding sessions as cards in its chat. The owner confirms each
// card (Send), edits it or cancels it. This file holds what the list rows, the chat and the profile share. The Chief
// shows only when `workspace.list` carries `chief.enabled`: an older desktop, a device narrowed to some projects or
// Monos (no `chief` key) and a Chief that is switched off all read as "nothing to show".
import type { ChiefCatalog, ChiefProposal } from "@monoptah/protocol";
import { CHIEF_CONVERSATION_ID } from "@monoptah/protocol";
import { useCatalog } from "@/data";

/** The route segment and the conversation id of the Chief. A real Mono id is generated, so it never equals this. */
export const CHIEF_ID = CHIEF_CONVERSATION_ID;
export const CHIEF_DEFAULT_NAME = "Chief";

/** The catalog entry of the Chief, or undefined when it must not show anywhere. */
export function useChiefCatalog(): ChiefCatalog | undefined {
  const chief = useCatalog().data?.chief;
  return chief?.enabled === true ? chief : undefined;
}

export const chiefName = (chief: ChiefCatalog | undefined): string => chief?.name ?? CHIEF_DEFAULT_NAME;

/** "1 command waiting for you" / "3 commands waiting for you". */
export const waitingLabel = (count: number): string => `${count} ${count === 1 ? "command" : "commands"} waiting for you`;

const ARROW = "→";
const DOT = "·";

/**
 * `<Mono name|Directly> -> #<n> <title> · <project> · <branch>`, with the arrow U+2192 and the middle dot U+00B7. A
 * proposal that starts new work reads `<Mono name|Directly> -> new session · <project>`.
 */
export function routeLine(proposal: ChiefProposal): string {
  const who = proposal.route.type === "viaMono" ? proposal.route.monoName : "Directly";
  if (proposal.kind === "start") return `${who} ${ARROW} new session ${DOT} ${proposal.project}`;
  const target = [proposal.sessionNumber !== undefined ? `#${proposal.sessionNumber}` : null, proposal.sessionTitle].filter(Boolean).join(" ");
  const parts = [target, proposal.project, proposal.branch].filter((part): part is string => !!part);
  return `${who} ${ARROW} ${parts.join(` ${DOT} `)}`;
}

const NOTE_TEXT: Record<string, string> = {
  sessionBusy: "Will wait in the session queue",
  monoBusy: "The Mono is busy and will take it after its current turn",
  cloudAsleep: "Waking the cloud sandbox costs money",
  queuePaused: "The session's queue is paused, so it waits until you resume it",
};

/** The text of one note; an unknown note of a newer desktop is dropped. */
export const noteText = (note: string): string | undefined => NOTE_TEXT[note];

/** The state chip of a proposal. */
export function stateText(proposal: ChiefProposal): string {
  const target = proposal.route.type === "viaMono" ? proposal.route.monoName : "the app";
  switch (proposal.state) {
    case "awaitingOwner":
      return "Waiting for your Send";
    case "queuedAtMono":
      return `Waiting for ${target}`;
    case "queuedInSession":
      return "Queued in session";
    case "sent":
      return "Sent";
    case "refused":
      return proposal.reason ? `Refused: ${proposal.reason}` : "Refused";
    case "notAttempted":
      return "Not sent";
    case "failed":
      return proposal.reason ? `Failed: ${proposal.reason}` : "Failed";
    case "cancelled":
      return "Cancelled";
    default:
      return proposal.state;
  }
}
