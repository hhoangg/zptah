// The state of the interactive visuals of one conversation (`visual.dispatch`, F-73). The desktop draws a visual and
// the web only has its tree, so a tapped control is sent to the desktop, which runs it in this device's own copy of
// the visual and answers the next tree. The controller owns that tree per visual: it survives the transcript's
// re-renders (and the virtual list unmounting a turn), goes back to the desktop's snapshot when the snapshot
// changes, sends one event at a time (a handler is only valid for the tree it came from), keeps the latest of the
// events that pile up for the same control while one is on its way, and ignores a reply that is out of date.
import { controlsOf, type ControlRef, type VisualEventValue } from "./controls";
import { parseVisualTree } from "./parse";
import type { VisualNode } from "./types";

export interface VisualRef {
  turnId: string;
  itemId: string;
  /** The visual document's id of the snapshot item. */
  documentId: string;
}

/** What the desktop answers: the tree after the event, or (`reset`) the first render when the handler was stale. */
export interface VisualReply {
  tree: unknown;
  reset: boolean;
}

export type VisualSend = (visual: VisualRef, handler: string, value: VisualEventValue) => Promise<VisualReply>;

export interface VisualView {
  /** The snapshot (JSON) this view started from; a different snapshot from the desktop starts the view over. */
  snapshot: string;
  node: VisualNode | null;
  pending: boolean;
  error: string | null;
}

interface Intent {
  place: number;
  kind: ControlRef["kind"];
  value: VisualEventValue;
}

interface Slot {
  view: VisualView;
  ref: VisualRef;
  /** Bumped when the snapshot changes: a reply of an earlier epoch is dropped. */
  epoch: number;
  /** The newest request sent and the newest reply applied; a reply older than the applied one is dropped. */
  sent: number;
  applied: number;
  busy: boolean;
  queue: Intent[];
  key: string;
}

const MAX_SLOTS = 64;

export class VisualController {
  readonly #send: VisualSend;
  readonly #slots = new Map<string, Slot>();
  readonly #listeners = new Map<string, Set<() => void>>();

  constructor(send: VisualSend) {
    this.#send = send;
  }

  /** The view of `key`, or undefined before the first `sync`. Stable until it changes (for `useSyncExternalStore`). */
  view(key: string): VisualView | undefined {
    return this.#slots.get(key)?.view;
  }

  subscribe(key: string, listener: () => void): () => void {
    const listeners = this.#listeners.get(key) ?? new Set();
    listeners.add(listener);
    this.#listeners.set(key, listeners);
    return () => {
      listeners.delete(listener);
      if (listeners.size === 0 && this.#listeners.get(key) === listeners) this.#listeners.delete(key);
    };
  }

  /** Makes the view of `key` match the desktop's snapshot: a changed snapshot drops what the controls did. */
  sync(key: string, ref: VisualRef, snapshot: string, tree: unknown): void {
    const slot = this.#slots.get(key);
    if (!slot) {
      if (this.#slots.size >= MAX_SLOTS) {
        // Only a visual nobody shows goes: a mounted one keeps its slot, or its taps would silently do nothing.
        for (const other of this.#slots.keys()) {
          if (!this.#listeners.has(other)) {
            this.#slots.delete(other);
            break;
          }
        }
      }
      this.#slots.set(key, {
        view: { snapshot, node: parseVisualTree(tree), pending: false, error: null },
        ref,
        epoch: 0,
        sent: 0,
        applied: 0,
        busy: false,
        queue: [],
        key,
      });
      this.#emit(this.#slots.get(key)!);
      return;
    }
    slot.ref = ref;
    if (slot.view.snapshot === snapshot) return;
    slot.epoch += 1;
    slot.queue = [];
    slot.busy = false;
    slot.view = { snapshot, node: parseVisualTree(tree), pending: false, error: null };
    this.#emit(slot);
  }

  /** A control was used: `handler` is the one of the tree the user saw. */
  dispatch(key: string, handler: string, value: VisualEventValue): void {
    const slot = this.#slots.get(key);
    const node = slot?.view.node;
    if (!slot || !node) return;
    const controls = controlsOf(node);
    const place = controls.findIndex((control) => control.handler === handler);
    const control = controls[place];
    if (!control) return;
    // The latest event of a control replaces an earlier one that has not been sent (a slider drag).
    slot.queue = slot.queue.filter((intent) => intent.place !== place);
    slot.queue.push({ place, kind: control.kind, value });
    this.#set(slot, { pending: true, error: null });
    void this.#pump(slot);
  }

  async #pump(slot: Slot): Promise<void> {
    if (slot.busy) return;
    slot.busy = true;
    const epoch = slot.epoch;
    try {
      while (slot.queue.length > 0 && slot.epoch === epoch) {
        const intent = slot.queue.shift()!;
        // The control is found again by its place: its handler changed with the last render.
        const control = slot.view.node ? controlsOf(slot.view.node)[intent.place] : undefined;
        if (!control || control.kind !== intent.kind) continue;
        const seq = ++slot.sent;
        let reply: VisualReply;
        try {
          reply = await this.#send(slot.ref, control.handler, intent.value);
        } catch (reason) {
          if (slot.epoch !== epoch) return;
          slot.queue = [];
          this.#set(slot, { pending: false, error: messageOf(reason) });
          return;
        }
        if (slot.epoch !== epoch) return;
        if (seq < slot.applied) continue;
        const node = parseVisualTree(reply.tree);
        if (!node) {
          slot.queue = [];
          this.#set(slot, { pending: false, error: "The desktop sent a visual this app cannot draw." });
          return;
        }
        slot.applied = seq;
        // After a reset the tree is the first render again: what was still waiting was meant for the old one.
        if (reply.reset) slot.queue = [];
        this.#set(slot, { node, error: null });
      }
    } finally {
      if (slot.epoch === epoch) {
        slot.busy = false;
        this.#set(slot, { pending: slot.queue.length > 0 });
      }
    }
  }

  #set(slot: Slot, change: Partial<Pick<VisualView, "node" | "pending" | "error">>): void {
    const next = { ...slot.view, ...change };
    if (next.node === slot.view.node && next.pending === slot.view.pending && next.error === slot.view.error) return;
    slot.view = next;
    this.#emit(slot);
  }

  #emit(slot: Slot): void {
    for (const listener of [...(this.#listeners.get(slot.key) ?? [])]) listener();
  }
}

function messageOf(reason: unknown): string {
  const code = (reason as { code?: unknown } | null)?.code;
  if (code === "busy") return "Too many taps at once. Try again in a moment.";
  if (code === "not_ready" || code === "session_ended") return "The desktop is not connected. Try again when it is back.";
  if (code === "unsupported") return "This desktop does not support controls in visuals yet. Open it on your desktop to interact.";
  return reason instanceof Error && reason.message ? reason.message : "The visual could not be updated.";
}
