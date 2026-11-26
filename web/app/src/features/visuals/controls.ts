// The interactive nodes of a visual tree and what a control may send back to the desktop (`visual.dispatch`).
import type { VisualNode } from "./types";

/** null (button), a boolean (toggle), text (input), an index (select, tabs) or a number (slider). */
export type VisualEventValue = null | boolean | string | number;

/** The text an input may send, in UTF-8 bytes (the desktop refuses more). */
export const MAX_INPUT_BYTES = 4096;

export type ControlKind = "button" | "toggle" | "input" | "select" | "slider" | "tabs";

export interface ControlRef {
  kind: ControlKind;
  handler: string;
}

const KINDS = new Set<string>(["button", "toggle", "input", "select", "slider", "tabs"]);

/** The controls of a tree in drawing order. A control keeps its place across renders while its handler changes. */
export function controlsOf(node: VisualNode): ControlRef[] {
  const found: ControlRef[] = [];
  const walk = (current: VisualNode) => {
    if (KINDS.has(current.kind)) {
      const control = current as Extract<VisualNode, { handler: string }>;
      found.push({ kind: control.kind as ControlKind, handler: control.handler });
    }
    if ("children" in current) for (const child of current.children) walk(child);
  };
  walk(node);
  return found;
}

const encoder = new TextEncoder();
export const utf8Length = (text: string): number => encoder.encode(text).length;
