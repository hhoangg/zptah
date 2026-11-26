// The hooks that tie a transcript's visual to the conversation's `VisualController`.
import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";
import { useLinkConnection } from "@/link";
import { useCapability } from "@/version";
import { parseVisualTree } from "./parse";
import type { VisualEventValue } from "./controls";
import { VisualController, type VisualRef } from "./VisualController";
import type { VisualNode } from "./types";

/**
 * The controller of one conversation, or undefined when the desktop does not advertise `visualControls` (an older
 * desktop answers the method as unsupported): the visuals then draw their controls disabled.
 */
export function useVisualController(conversationId: string): VisualController | undefined {
  const client = useLinkConnection();
  const supported = useCapability("visualControls");
  return useMemo(
    () =>
      supported
        ? new VisualController(async (visual, handler, value) => {
            const reply = await client.request("visual.dispatch", { conversationId, turnId: visual.turnId, itemId: visual.itemId, documentId: visual.documentId, handler, value });
            return { tree: reply.tree, reset: reply.reset };
          })
        : undefined,
    [client, conversationId, supported],
  );
}

export interface InteractiveVisual {
  node: VisualNode | null;
  pending: boolean;
  error: string | null;
  /** Undefined without a controller: the controls are shown but cannot act. */
  onDispatch: ((handler: string, value: VisualEventValue) => void) | undefined;
}

/** The tree to draw for a snapshot, with the controls wired to `controller` when there is one. */
export function useVisual(controller: VisualController | undefined, key: string, ref: VisualRef, snapshot: unknown): InteractiveVisual {
  const json = useMemo(() => JSON.stringify(snapshot) ?? "", [snapshot]);
  const parsed = useMemo(() => parseVisualTree(snapshot), [snapshot]);
  const subscribe = useCallback((listener: () => void) => controller?.subscribe(key, listener) ?? (() => {}), [controller, key]);
  const read = useCallback(() => controller?.view(key), [controller, key]);
  const view = useSyncExternalStore(subscribe, read, read);
  // The controller learns the snapshot after the render; until it has (or when the snapshot just changed) the
  // snapshot itself is drawn, so a new snapshot is never shown through an old tree.
  useEffect(() => {
    controller?.sync(key, ref, json, snapshot);
  }, [controller, key, ref, json, snapshot]);
  const onDispatch = useCallback((handler: string, value: VisualEventValue) => controller?.dispatch(key, handler, value), [controller, key]);
  if (!controller) return { node: parsed, pending: false, error: null, onDispatch: undefined };
  const current = view && view.snapshot === json ? view : undefined;
  return { node: current ? current.node : parsed, pending: current?.pending ?? false, error: current?.error ?? null, onDispatch };
}
