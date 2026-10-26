// The requests of one conversation that go through TanStack Query (plan section 2.2):
// the pages of older history, and the mutations of the conversation screen. The live
// stream does not go through Query; it has its own store.
//
// - Keys start with `machineKey(machine)`, so Forget removes them with the machine.
// - The answers, steers and withdraws of one conversation share one scope, so they
//   reach efrd in the order of the taps. Stop (`turn.interrupt`) has no scope, so it
//   never waits behind a stuck call.
// - Each write carries a `command_id` from its variables. The caller makes one id per
//   tap (expo-crypto); to send a failed write again, it calls `mutate` with the same
//   variables, and efrd runs it only once.
// - A screen spreads a factory and adds its own `onSuccess` or `onError`. The factories
//   use `onSettled` to refresh the lists; a screen that sets `onSettled` must do that
//   itself.
// - The app never answers "always allow": efrd refuses it from a phone.

import { infiniteQueryOptions, mutationOptions } from "@tanstack/react-query";

import { conversationsKeys } from "@/features/conversations/conversations-queries";
import { conversationScope, machineKey, type ConversationAddress } from "@/lib/query/keys";
import type { MachineCaller } from "@/lib/query/machine-caller";
import type { TurnInterrupt, TurnSteer } from "@/lib/protocol/generated";
import type { HistoryPage } from "@/lib/protocol/wire";

/** The most history pages in memory. Older pages past it are fetched again on scroll. */
export const MAX_HISTORY_PAGES = 12;

/** The query and mutation keys of one conversation. */
export const conversationKeys = {
  conversation: (ref: ConversationAddress) =>
    [...machineKey(ref.machine), "conversation", ref.id] as const,
  /** The history pages before `start` (a snapshot's `history_cursor`; null: the newest). */
  history: (ref: ConversationAddress, start: string | null) =>
    [...machineKey(ref.machine), "conversation", ref.id, "history", start] as const,
  approvalAnswers: (ref: ConversationAddress) =>
    [...machineKey(ref.machine), "conversation", ref.id, "approval.respond"] as const,
  steer: (ref: ConversationAddress) =>
    [...machineKey(ref.machine), "conversation", ref.id, "turn.steer"] as const,
  interrupt: (ref: ConversationAddress) =>
    [...machineKey(ref.machine), "conversation", ref.id, "turn.interrupt"] as const,
  withdraw: (ref: ConversationAddress) =>
    [...machineKey(ref.machine), "conversation", ref.id, "prompt.withdraw"] as const,
};

/**
 * The page param of a history page. `conversation.history` pages go only to older
 * events, so each param also holds the param of the newer page next to it. When
 * MAX_HISTORY_PAGES drops the newest page from memory, `fetchPreviousPage` gets it again
 * with that param.
 */
export interface HistoryPageParam {
  /** The cursor to send; null asks for the newest page. */
  readonly cursor: string | null;
  /** The param of the newer page; absent for the first page. */
  readonly newer?: HistoryPageParam;
}

/**
 * The history of `ref` before `start`, a page at a time. `fetchNextPage` gets older
 * events, `fetchPreviousPage` newer ones that were dropped. Each page holds its events
 * oldest first.
 */
export function historyQueryOptions(
  caller: MachineCaller,
  ref: ConversationAddress,
  start: string | null,
) {
  const first: HistoryPageParam = { cursor: start };
  return infiniteQueryOptions({
    queryKey: conversationKeys.history(ref, start),
    queryFn: ({ pageParam, signal }) =>
      caller.call(
        ref.machine,
        "conversation.history",
        { conversation_id: ref.id, cursor: pageParam.cursor },
        signal,
      ),
    initialPageParam: first,
    getNextPageParam: (
      page: HistoryPage,
      _pages: HistoryPage[],
      param: HistoryPageParam,
    ): HistoryPageParam | undefined =>
      page.next_cursor === undefined || page.next_cursor === null
        ? undefined
        : { cursor: page.next_cursor, newer: param },
    getPreviousPageParam: (
      _page: HistoryPage,
      _pages: HistoryPage[],
      param: HistoryPageParam,
    ): HistoryPageParam | undefined => param.newer,
    maxPages: MAX_HISTORY_PAGES,
  });
}

/** An answer that the phone can send: never "always allow". */
export type PhoneDecision = "allow" | "deny";

/** The variables of an approval answer. */
export interface ApprovalAnswer {
  readonly callId: string;
  readonly decision: PhoneDecision;
  readonly commandId: string;
}

/** The variables of a steer. */
export interface SteerInput {
  readonly text: string;
  readonly commandId: string;
  /** The running turn as the screen sees it; a steer for another turn is late. */
  readonly turnId?: string;
}

/** The variables of Stop. */
export interface InterruptInput {
  readonly commandId: string;
  /** The running turn as the screen sees it; efrd stops no other turn. */
  readonly turnId?: string;
}

/** The variables of a withdraw of a queued prompt. */
export interface WithdrawInput {
  readonly commandId: string;
  /** The turn of the queued prompt. */
  readonly turnId: string;
}

/** Answers an approval of `ref`. */
export function answerApprovalMutationOptions(caller: MachineCaller, ref: ConversationAddress) {
  return mutationOptions({
    mutationKey: conversationKeys.approvalAnswers(ref),
    mutationFn: (answer: ApprovalAnswer) =>
      caller.call(ref.machine, "approval.respond", {
        call_id: answer.callId,
        command_id: answer.commandId,
        conversation_id: ref.id,
        decision: answer.decision,
      }),
    scope: conversationScope(ref),
    onSettled: (_data, _error, _answer, _result, context) => {
      void context.client.invalidateQueries({ queryKey: conversationsKeys.lists(ref.machine) });
    },
  });
}

/**
 * Steers the running turn of `ref`. A late steer (the turn ends first) becomes a
 * queued prompt, not a `conflict` (`if_late: queue`).
 */
export function steerMutationOptions(caller: MachineCaller, ref: ConversationAddress) {
  return mutationOptions({
    mutationKey: conversationKeys.steer(ref),
    mutationFn: (input: SteerInput) => {
      const params: TurnSteer = {
        command_id: input.commandId,
        conversation_id: ref.id,
        text: input.text,
        if_late: { kind: "queue" },
      };
      if (input.turnId !== undefined) {
        params.turn_id = input.turnId;
      }
      return caller.call(ref.machine, "turn.steer", params);
    },
    scope: conversationScope(ref),
  });
}

/** Stops the running turn of `ref`. No scope: it never waits behind another write. */
export function interruptMutationOptions(caller: MachineCaller, ref: ConversationAddress) {
  return mutationOptions({
    mutationKey: conversationKeys.interrupt(ref),
    mutationFn: (input: InterruptInput) => {
      const params: TurnInterrupt = { command_id: input.commandId, conversation_id: ref.id };
      if (input.turnId !== undefined) {
        params.turn_id = input.turnId;
      }
      return caller.call(ref.machine, "turn.interrupt", params);
    },
    onSettled: (_data, _error, _input, _result, context) => {
      void context.client.invalidateQueries({ queryKey: conversationsKeys.lists(ref.machine) });
    },
  });
}

/** Takes back a queued prompt of `ref`. */
export function withdrawPromptMutationOptions(caller: MachineCaller, ref: ConversationAddress) {
  return mutationOptions({
    mutationKey: conversationKeys.withdraw(ref),
    mutationFn: (input: WithdrawInput) =>
      caller.call(ref.machine, "prompt.withdraw", {
        command_id: input.commandId,
        conversation_id: ref.id,
        target: { kind: "turn", turn_id: input.turnId },
      }),
    scope: conversationScope(ref),
  });
}

/** The filters of `useMutationState` for the answers of `ref` that are on their way. */
export function pendingAnswerFilters(ref: ConversationAddress) {
  return { mutationKey: conversationKeys.approvalAnswers(ref), status: "pending" as const };
}

/** True when `value` is the variables of an approval answer. */
export function isApprovalAnswer(value: unknown): value is ApprovalAnswer {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const callId: unknown = Reflect.get(value, "callId");
  const decision: unknown = Reflect.get(value, "decision");
  const commandId: unknown = Reflect.get(value, "commandId");
  return (
    typeof callId === "string" &&
    typeof commandId === "string" &&
    (decision === "allow" || decision === "deny")
  );
}

/**
 * The decision on its way for approval `callId`, from the variables that
 * `useMutationState` gives with `pendingAnswerFilters`: the Allow button shows the
 * braille cell, or Deny dims. The newest answer wins.
 */
export function pendingDecision(
  variables: readonly unknown[],
  callId: string,
): PhoneDecision | undefined {
  let decision: PhoneDecision | undefined;
  for (const value of variables) {
    if (isApprovalAnswer(value) && value.callId === callId) {
      decision = value.decision;
    }
  }
  return decision;
}
