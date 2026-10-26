// What the stack does after a Forget in Settings (design section 1, "Stack rules"):
//
// - The last machine: Settings goes away and the stack resets to the root, which then
//   shows the pairing intro, because no machine is left.
// - Another machine, with conversations of it in the stack: the stack goes back to the
//   blank root (the root then opens on a machine that is still paired). Settings stays
//   on top, so the user sees the row leave the list.
// - Another machine, with nothing of it in the stack: nothing moves.
//
// Pure, for the tests. The screen applies the plan with `navigation.reset`.

import type { MachineId } from "@/features/machines/model/machine-ref";

/** A route of the stack as the plan sees it: its name and its params. */
export interface PlanRoute {
  readonly name: string;
  readonly params?: object | undefined;
}

/** What the stack does. `keep` are the indexes of the routes that stay, in order. */
export type ForgetPlan =
  | { readonly kind: "stay" }
  | { readonly kind: "reset"; readonly keep: readonly number[]; readonly fade: boolean };

/** The route of a conversation: `c/[machine]/[id]`. */
const CONVERSATION_ROUTE = "c/[machine]/[id]";

/**
 * The plan after the user forgot `machine`. `routes` is the stack, root first, with
 * Settings on top; `remaining` is the number of machines that are still paired.
 */
export function forgetPlan(
  routes: readonly PlanRoute[],
  machine: MachineId,
  remaining: number,
): ForgetPlan {
  if (routes.length === 0) {
    return { kind: "stay" };
  }
  if (remaining === 0) {
    return { kind: "reset", keep: [0], fade: true };
  }
  const top = routes.length - 1;
  if (!routes.some((route) => isConversationOf(route, machine))) {
    return { kind: "stay" };
  }
  return { kind: "reset", keep: top === 0 ? [0] : [0, top], fade: false };
}

function isConversationOf(route: PlanRoute, machine: MachineId): boolean {
  const params = route.params;
  return (
    route.name === CONVERSATION_ROUTE &&
    params !== undefined &&
    "machine" in params &&
    params.machine === machine
  );
}
