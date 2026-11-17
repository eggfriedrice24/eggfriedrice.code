// The row state of a conversation in a list (spec 0.0.12, section 6.1): who can act on
// it now. It comes from the two members of a `ConversationSummary`, `status` and
// `attention`, with one rule for every client, so the phone and `efr` agree. Pure.
//
// The order counts: a waiting approval that the phone can answer comes first, then a
// question of the model, then a wait that only the user's own machine can answer, then
// a running turn, then a failed last turn. Anything else is idle.

import type { Attention, ConversationStatus } from "./generated";

/** What a list row says about a conversation. */
export type RowState = "approval" | "answer" | "terminal" | "running" | "failed" | "idle";

/** The row states of a conversation that waits for the user. */
export type WaitState = Extract<RowState, "approval" | "answer" | "terminal">;

/**
 * The row state of a conversation. `canAnswer` is true when this client can answer a
 * question of the model on that machine. Without it, a question reads as a wait for the
 * terminal, so a row never asks for an answer that the client cannot give. An
 * `attention` that this build does not know (`unknown`) reads as none.
 */
export function rowState(
  status: ConversationStatus,
  attention: Attention | null | undefined,
  canAnswer: boolean,
): RowState {
  if (status === "awaiting_approval" && attention !== "terminal") {
    return "approval";
  }
  if (attention === "answer") {
    return canAnswer ? "answer" : "terminal";
  }
  if (attention === "terminal") {
    return "terminal";
  }
  if (status === "running") {
    return "running";
  }
  return attention === "failed" ? "failed" : "idle";
}

/** True when the conversation waits for the user: an approval, an answer or the terminal. */
export function waitsForUser(state: RowState): state is WaitState {
  return state === "approval" || state === "answer" || state === "terminal";
}
