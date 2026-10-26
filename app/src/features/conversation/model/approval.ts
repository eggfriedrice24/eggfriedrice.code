// The rules of a phone answer to an approval question, on the one approval type of the
// stream reducer. model/approval-block.ts uses them for what the block shows.
//
// The phone answers only `allow` and `deny` (R3.10 of the research): it never offers
// "always", and it sends no answer to a question that only the user's own machine may
// answer (`local_only`); efrd refuses such an answer from a phone and keeps the question
// open.

import type { ApprovalDecision } from "@/lib/protocol/generated";
import type { ApprovalView } from "@/lib/stream/reducer";

/** An answer that the phone can give. */
export type PhoneDecision = "allow" | "deny";

/** Why the phone cannot give an answer. */
export type AnswerRefusal = "unknown" | "not_pending" | "local_only" | "decision";

/**
 * Why the phone must not send `decision` for `approval`, or `undefined` when it may. The
 * mutation checks it before the call, so the block and the call read the same state.
 */
export function answerRefusal(
  approval: ApprovalView | undefined,
  decision: ApprovalDecision,
): AnswerRefusal | undefined {
  if (approval === undefined) {
    return "unknown";
  }
  if (approval.status !== "pending") {
    return "not_pending";
  }
  if (approval.localOnly) {
    return "local_only";
  }
  return decision === "allow" || decision === "deny" ? undefined : "decision";
}

/** Why only the user's own machine may answer a question. */
export type LocalReason = "privilege" | "delete" | "job";

/**
 * Why `approval` waits for an answer on the computer, or `undefined` when the phone may
 * answer. efrd sets `local_only` for a privilege exit or a command that runs as another
 * user, for a delete that full mode guards (it has a fixed limit) and for a job's
 * question.
 */
export function localReason(approval: ApprovalView): LocalReason | undefined {
  if (!approval.localOnly) {
    return undefined;
  }
  if (approval.exitKinds.includes("privilege") || approval.interactive) {
    return "privilege";
  }
  if (approval.expiresAt !== undefined || approval.exitKinds.includes("destructive")) {
    return "delete";
  }
  return "job";
}
