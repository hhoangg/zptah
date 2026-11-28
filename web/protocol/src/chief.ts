// The Chief: a special Mono that proposes commands for coding sessions as proposal cards. The owner confirms
// (Send), edits or cancels each card. Wire shapes of `chief.*`, the `chief` extra of `conversation.read` and watch
// frames, and the `chief` key of `workspace.list`. Results stay open (looseObject) so a newer desktop can add fields.
import "./zod-config.ts";
import { z } from "zod";

export const CHIEF_CONVERSATION_ID = "chief";

export const CHIEF_NOTES = ["sessionBusy", "monoBusy", "cloudAsleep", "queuePaused"] as const;
export const CHIEF_STATES = ["awaitingOwner", "cancelled", "queuedAtMono", "queuedInSession", "sent", "refused", "notAttempted", "failed"] as const;

export const ChiefRouteSchema = z.union([
  z.looseObject({ type: z.literal("viaMono"), monoId: z.string(), monoName: z.string() }),
  z.looseObject({ type: z.literal("direct") }),
]);

export const ChiefProposalSchema = z.looseObject({
  id: z.string(),
  createdAt: z.number(),
  confirmedAt: z.number().optional(),
  route: ChiefRouteSchema,
  sessionNumber: z.number().optional(),
  sessionTitle: z.string(),
  project: z.string(),
  branch: z.string().optional(),
  instruction: z.string(),
  notes: z.array(z.string()),
  state: z.string(),
  reason: z.string().optional(),
  /** True only while the state is `awaitingOwner`. */
  open: z.boolean(),
  kind: z.string(),
});
export type ChiefProposal = z.infer<typeof ChiefProposalSchema>;

/** The `chief` extra of `conversation.read` and watch frames (frames carry it only on change). */
export const ChiefExtraSchema = z.looseObject({
  openCount: z.number(),
  allowWebSend: z.boolean(),
  proposals: z.array(ChiefProposalSchema),
});
export type ChiefExtra = z.infer<typeof ChiefExtraSchema>;

/** The `chief` key of `workspace.list`: absent on an older desktop and on a device narrowed to some projects or Monos. */
export const ChiefCatalogSchema = z.looseObject({
  enabled: z.boolean(),
  name: z.string().optional(),
  mascot: z.string().optional(),
  emoji: z.string().optional(),
  color: z.string().optional(),
  status: z.string().optional(),
  openCount: z.number(),
});
export type ChiefCatalog = z.infer<typeof ChiefCatalogSchema>;

export const ChiefProposalsResultSchema = z.looseObject({
  enabled: z.boolean(),
  proposals: z.array(ChiefProposalSchema),
  openCount: z.number(),
  allowWebSend: z.boolean(),
});
export type ChiefProposalsResult = z.infer<typeof ChiefProposalsResultSchema>;

export const ChiefProfileResultSchema = z.looseObject({
  enabled: z.boolean(),
  name: z.string(),
  mascot: z.string(),
  emoji: z.string().optional(),
  color: z.string(),
  status: z.string(),
  openCount: z.number(),
  allowWebSend: z.boolean(),
  notifyOnDelivery: z.boolean(),
  harness: z.looseObject({ id: z.string(), model: z.string().optional(), effort: z.string().optional(), ok: z.boolean(), line: z.string() }),
});
export type ChiefProfileResult = z.infer<typeof ChiefProfileResultSchema>;

export const ChiefProposalResultSchema = z.looseObject({ proposal: ChiefProposalSchema });
