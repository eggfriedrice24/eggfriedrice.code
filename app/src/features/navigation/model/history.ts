// The history rules of the stack (plan section 2.5, design section 1 "Stack rules"). Pure:
// `planOpen` says how to open a conversation, and use-open-conversation.ts does it with
// the navigation of the root stack.
//
// - The root (a blank conversation, or the pairing intro) is always at index 0.
// - A conversation that is already in the stack is not pushed again: the stack pops back
//   to it and drops everything above it.
// - At most MAX_STACKED conversations are above the root. The push of one more first
//   drops the oldest (a reset without an animation), so the new one ends on top.
// - A screen that is not a conversation on top of the stack (the All conversations sheet)
//   goes away before the push, because a pick in the sheet dismisses the sheet.

import type { ConversationRef, MachineId } from "@/features/machines/model/machine-ref";

/** The most conversations above the root. */
export const MAX_STACKED = 8;

/** One route of the stack, as the history rules see it. */
export type StackRoute =
  | { readonly name: "index" }
  | { readonly name: "c"; readonly machine: MachineId; readonly id: string }
  /** A screen that is not a conversation, such as Settings or the sheet. */
  | { readonly name: "other" };

/** How to open a conversation. */
export type OpenPlan =
  /** The target is at `index` already: pop the routes above it. */
  | { readonly kind: "pop-to"; readonly index: number }
  /**
   * Push the target. Before it, pop the `dismiss` routes on top that are not
   * conversations, and drop the `trim` oldest routes above the root. `instant` is the
   * push of the first send on the blank root, which has no animation.
   */
  | {
      readonly kind: "push";
      readonly instant: boolean;
      readonly dismiss: number;
      readonly trim: number;
    };

function isTarget(route: StackRoute, target: ConversationRef): boolean {
  return route.name === "c" && route.machine === target.machine && route.id === target.id;
}

/** How to open `target` on `stack` (the routes of the root stack, the root first). */
export function planOpen(
  stack: readonly StackRoute[],
  target: ConversationRef,
  options: { readonly firstSend: boolean },
): OpenPlan {
  for (let index = stack.length - 1; index >= 1; index -= 1) {
    const route = stack[index];
    if (route !== undefined && isTarget(route, target)) {
      return { kind: "pop-to", index };
    }
  }
  let top = stack.length;
  while (top > 1 && stack[top - 1]?.name === "other") {
    top -= 1;
  }
  const above = stack.slice(1, top);
  const conversations = above.filter((route) => route.name === "c").length;
  let excess = conversations + 1 - MAX_STACKED;
  let trim = 0;
  while (excess > 0 && trim < above.length) {
    if (above[trim]?.name === "c") {
      excess -= 1;
    }
    trim += 1;
  }
  return { kind: "push", instant: options.firstSend, dismiss: stack.length - top, trim };
}

/** The stack after `plan` opened `target`: what the navigation must end with. */
export function applyPlan(
  stack: readonly StackRoute[],
  target: ConversationRef,
  plan: OpenPlan,
): StackRoute[] {
  if (plan.kind === "pop-to") {
    return stack.slice(0, plan.index + 1);
  }
  const kept = stack.slice(0, stack.length - plan.dismiss);
  const root = kept[0] ?? { name: "index" };
  return [
    root,
    ...kept.slice(1 + plan.trim),
    { name: "c", machine: target.machine, id: target.id },
  ];
}
