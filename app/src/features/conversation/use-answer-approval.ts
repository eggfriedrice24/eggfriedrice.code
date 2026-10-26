// The hooks of an approval block: the answer of the phone (Allow or Deny) as a mutation,
// and the countdown of a question with a time limit.
//
// - The answer goes through `answerApprovalMutationOptions`, so all answers, steers and
//   withdraws of a conversation reach efrd in order. The state of the answer comes from
//   the mutation cache (`useMutationState`), not from `useState`: a row that the list
//   recycles, or a second view of the same conversation, sees the same state.
// - efrd's first answer wins, from any client. So the block keeps Allow on the braille
//   cell after efrd took the answer, until `approval_resolved` arrives in the stream.
// - A tap gives the medium haptic (design section 5). A tap that efrd would refuse
//   (`answerRefusal`) sends nothing.
// - A resend after a failed answer uses the command id of that answer, so efrd runs the
//   answer at most once.
// - The countdown is an external store over a clock: no effect and no state of its own.

import { useMutation, useMutationState } from "@tanstack/react-query";
import { useSyncExternalStore } from "react";

import type { Clock } from "@/lib/net/clock";
import { systemClock } from "@/lib/native/clock";
import { newCommandId } from "@/lib/native/database";
import { haptic } from "@/lib/native/haptics";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { ApprovalView } from "@/lib/stream/reducer";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import {
  answerState,
  NO_ANSWER,
  type AnswerMutation,
  type AnswerState,
} from "./components/approval-block-model";
import { expiryLabel, watchExpiry } from "./components/approval-block-text";
import {
  answerApprovalMutationOptions,
  conversationKeys,
  type PhoneDecision,
} from "./conversation-queries";
import { answerRefusal } from "./model/approval";

/** What `useAnswerApproval` needs. */
export interface AnswerApprovalOptions {
  /** The runtime, or a fake in tests. */
  readonly caller: MachineCaller;
  readonly conversation: ConversationRef;
  /** The question as the stream shows it now; `undefined` while it is not loaded. */
  readonly approval: ApprovalView | undefined;
  /** Makes the command id of a new answer. The default is a v4 UUID of expo-crypto. */
  readonly newId?: () => string;
}

/** The answer of the phone to one question. */
export interface AnswerApproval {
  readonly state: AnswerState;
  /** Sends `decision`, when the phone may send it now. */
  readonly answer: (decision: PhoneDecision) => void;
}

/** The answer of the phone to `options.approval`. */
export function useAnswerApproval(options: AnswerApprovalOptions): AnswerApproval {
  const { approval, conversation } = options;
  const mutation = useMutation(answerApprovalMutationOptions(options.caller, conversation));
  const mutations = useMutationState({
    filters: { mutationKey: conversationKeys.approvalAnswers(conversation) },
    select: (entry): AnswerMutation => ({
      variables: entry.state.variables,
      status: entry.state.status,
      error: entry.state.error,
      submittedAt: entry.state.submittedAt,
    }),
  });
  const state =
    approval === undefined
      ? NO_ANSWER
      : answerState(mutations, approval.callId, approval.status === "pending");
  const answer = (decision: PhoneDecision): void => {
    if (
      approval === undefined ||
      state.sending !== undefined ||
      answerRefusal(approval, decision) !== undefined
    ) {
      return;
    }
    haptic("medium");
    const failed = state.failed;
    const commandId =
      failed !== undefined && failed.decision === decision
        ? failed.commandId
        : (options.newId ?? newCommandId)();
    mutation.mutate({ callId: approval.callId, decision, commandId });
  };
  return { state, answer };
}

/**
 * The header text of the time to answer (`expires in 4m`, `expires in 45s`, `time is
 * up`), or `undefined` for a question without a limit. The component renders again only
 * when the text changes.
 */
export function useExpiryLabel(
  expiresAt: number | undefined,
  clock: Clock = systemClock,
): string | undefined {
  const subscribe = (onChange: () => void): (() => void) =>
    expiresAt === undefined ? () => undefined : watchExpiry(expiresAt, clock, onChange);
  const snapshot = (): string | undefined =>
    expiresAt === undefined ? undefined : expiryLabel(expiresAt, clock.now());
  return useSyncExternalStore(subscribe, snapshot);
}
