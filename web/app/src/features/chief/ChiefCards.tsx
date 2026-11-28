// The proposal cards of the Chief chat. Placement mirrors the desktop (`run_proposals`): a card is drawn right after
// the turn whose `chief.propose` tool result returned its id (the turn lists the ids in `chiefProposals`). A proposal
// whose turn is not loaded (or an older desktop that lists none) falls back to one list under the transcript, above the
// composer, oldest first so the newest card sits last. A proposal is never drawn twice. The cards come from the
// `chief` extra of the conversation read and watch frames, refreshed with `chief.proposals` when the chat opens; a
// result of Send, Edit or Cancel shows at once and is replaced by the next frame.
//
// Send needs a Control device AND the desktop's "Allow Send from the web" toggle. Without both, the card says "Confirm on the
// desktop" and has no Send button. Edit and Cancel need Control and are left out for a read-only device.
import type { ChiefProposal, Turn } from "@monoptah/protocol";
import { createContext, use, useEffect, useMemo, useState, type ReactNode } from "react";
import { useChiefExtra, useLinkMutation, useLinkQuery } from "@/data";
import { relativeTime } from "@/features/sessions/relativeTime";
import { useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { readProposalIds } from "@/features/transcript/turnItems";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { noteText, routeLine, stateText, waitingLabel, CHIEF_ID } from "./chief";

interface ChiefProposalsValue {
  /** Newest first, with this device's latest answers applied. */
  proposals: ChiefProposal[];
  /** Proposals drawn inline after a loaded turn. */
  placed: ReadonlySet<string>;
  card: (proposal: ChiefProposal) => ReactNode;
}

const ChiefProposalsContext = createContext<ChiefProposalsValue | null>(null);

/**
 * Owns the Chief's proposals for its chat: the data, the answers to this device's clicks and the card actions. `turns`
 * are the loaded turns of the transcript, which decide which proposals can sit inline.
 */
export function ChiefProposalsProvider({ turns, children }: { turns: readonly Turn[]; children: ReactNode }) {
  const readOnly = useReadOnly();
  const send = useLinkMutation("chief.proposal.send");
  const edit = useLinkMutation("chief.proposal.edit");
  const cancel = useLinkMutation("chief.proposal.cancel");
  const extra = useChiefExtra(CHIEF_ID);
  const query = useLinkQuery("chief.proposals", {});
  const { refetch } = query;
  // The chat opening is the moment to ask again: the query itself never goes stale on its own.
  useEffect(() => {
    void refetch();
  }, [refetch]);
  const source = extra ?? query.data;
  // Answers of this device's clicks, keyed by the data they were made against.
  const [answers, setAnswers] = useState<{ base: unknown; byId: Record<string, ChiefProposal> }>({ base: undefined, byId: {} });
  const base = source;
  const remember = (proposal: ChiefProposal) => setAnswers((current) => ({ base, byId: { ...(current.base === base ? current.byId : {}), [proposal.id]: proposal } }));
  const proposals = useMemo(
    () => (source ? source.proposals.map((proposal) => (answers.base === source ? answers.byId[proposal.id] : undefined) ?? proposal) : []),
    [source, answers],
  );
  const placed = useMemo(() => {
    const known = new Set(proposals.map((proposal) => proposal.id));
    return new Set(turns.flatMap((turn) => readProposalIds(turn)).filter((id) => known.has(id)));
  }, [proposals, turns]);
  const allowWebSend = source?.allowWebSend === true;
  const value: ChiefProposalsValue = {
    proposals,
    placed,
    card: (proposal) => (
      <ProposalCard
        key={proposal.id}
        proposal={proposal}
        canControl={!readOnly}
        canSend={!readOnly && allowWebSend}
        onSend={(instruction) => send.mutateAsync({ proposalId: proposal.id, expectedInstruction: proposal.instruction, ...(instruction !== undefined ? { instruction } : {}) }).then((result) => remember(result.proposal))}
        onEdit={(instruction) => edit.mutateAsync({ proposalId: proposal.id, instruction }).then((result) => remember(result.proposal))}
        onCancel={() => cancel.mutateAsync({ proposalId: proposal.id }).then((result) => remember(result.proposal))}
      />
    ),
  };
  return <ChiefProposalsContext value={value}>{children}</ChiefProposalsContext>;
}

/** The cards of the proposals one turn made, drawn right after it. */
export function ChiefInlineCards({ ids }: { ids: readonly string[] }) {
  const value = use(ChiefProposalsContext);
  if (!value || ids.length === 0) return null;
  const cards = ids.flatMap((id) => value.proposals.find((proposal) => proposal.id === id) ?? []);
  if (cards.length === 0) return null;
  return (
    <div data-testid="chief-inline-cards" className="flex flex-col gap-2 px-5 py-2">
      {cards.map((proposal) => value.card(proposal))}
    </div>
  );
}

/** The fallback list under the transcript: every proposal that is not drawn inline after its turn. */
export function ChiefCards() {
  const value = use(ChiefProposalsContext);
  if (!value) return null;
  const rest = value.proposals.filter((proposal) => !value.placed.has(proposal.id));
  if (rest.length === 0) return null;
  const openCount = rest.filter((proposal) => proposal.open).length;
  // Oldest first: the newest card is the last one, next to the composer.
  const ordered = [...rest].reverse();

  return (
    <section aria-label="Commands" data-testid="chief-cards" className="mx-auto flex max-h-[45%] w-full max-w-[768px] shrink-0 flex-col gap-2 overflow-y-auto px-3 py-2">
      <h2 className="px-1 text-label font-medium text-muted-text">{openCount > 0 ? waitingLabel(openCount) : "Commands"}</h2>
      {ordered.map((proposal) => value.card(proposal))}
    </section>
  );
}

const messageOf = (error: unknown): string => (error instanceof Error && error.message ? error.message : "Something went wrong. Try again.");

export function ProposalCard({
  proposal,
  canControl,
  canSend,
  onSend,
  onEdit,
  onCancel,
}: {
  proposal: ChiefProposal;
  /** A Control device may edit and cancel. */
  canControl: boolean;
  /** Control AND the desktop's "Allow Send from the web". */
  canSend: boolean;
  onSend: (instruction: string | undefined) => Promise<unknown>;
  onEdit: (instruction: string) => Promise<unknown>;
  onCancel: () => Promise<unknown>;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(proposal.instruction);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const notes = proposal.notes.map(noteText).filter((text): text is string => !!text);

  const run = async (action: () => Promise<unknown>, done?: () => void) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      done?.();
    } catch (failure) {
      setError(messageOf(failure));
    } finally {
      setBusy(false);
    }
  };

  const edited = editing && draft.trim() !== "" && draft !== proposal.instruction;
  return (
    <article data-testid="chief-card" data-state={proposal.state} aria-label={routeLine(proposal)} className="flex flex-col gap-2 rounded-card border border-stroke bg-surface p-3">
      <p className="text-label font-medium text-muted-text wrap-anywhere">{routeLine(proposal)}</p>
      {editing ? (
        <textarea
          aria-label="Command text"
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          rows={4}
          className="w-full resize-y rounded-[12px] border border-stroke bg-transparent p-2 text-body"
        />
      ) : (
        <p className="text-body whitespace-pre-wrap wrap-anywhere select-text">{proposal.instruction}</p>
      )}
      {proposal.open && notes.length > 0 ? (
        <ul className="flex flex-col gap-0.5 text-small text-caution">
          {notes.map((text) => (
            <li key={text}>{text}</li>
          ))}
        </ul>
      ) : null}
      <div className="flex flex-wrap items-center gap-2">
        <span data-testid="chief-state" className={cn("rounded-pill bg-fill px-2.5 py-0.5 text-meta", (proposal.state === "failed" || proposal.state === "refused") && "text-deletions")}>
          {stateText(proposal)}
        </span>
        <span className="text-meta text-muted-text">{relativeTime(proposal.createdAt)}</span>
      </div>
      {proposal.open ? (
        <div className="flex flex-wrap items-center gap-2">
          {canSend ? (
            <Button type="button" size="sm" disabled={busy || (editing && draft.trim() === "")} onClick={() => void run(() => onSend(edited ? draft : undefined))}>
              Send
            </Button>
          ) : (
            <span data-testid="chief-confirm-desktop" className="text-small text-muted-text">
              Confirm on the desktop
            </span>
          )}
          {canControl && editing ? (
            <>
              <Button type="button" size="sm" variant="outline" disabled={busy || draft.trim() === "" || draft === proposal.instruction} onClick={() => void run(() => onEdit(draft), () => setEditing(false))}>
                Save
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                disabled={busy}
                onClick={() => {
                  setEditing(false);
                  setDraft(proposal.instruction);
                  setError(null);
                }}
              >
                Discard
              </Button>
            </>
          ) : canControl ? (
            <>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => {
                  setDraft(proposal.instruction);
                  setEditing(true);
                }}
              >
                Edit
              </Button>
              <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={() => void run(onCancel)}>
                Cancel
              </Button>
            </>
          ) : null}
        </div>
      ) : null}
      {error ? (
        <p role="alert" data-testid="chief-card-error" className="text-small text-deletions">
          {error}
        </p>
      ) : null}
    </article>
  );
}
