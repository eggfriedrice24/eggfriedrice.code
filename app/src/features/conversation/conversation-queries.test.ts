import {
  InfiniteQueryObserver,
  MutationObserver,
  type InfiniteData,
  type QueryClient,
} from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { conversationsKeys } from "@/features/conversations/conversations-queries";
import type {
  ApprovalRespondResult,
  ConversationHistory,
  ConversationHistoryResult,
} from "@/lib/protocol/generated";
import type { HistoryPage } from "@/lib/protocol/wire";
import { machineKey } from "@/lib/query/keys";
import { createQueryClient } from "@/lib/query/query-client";
import { FakeCaller } from "@/lib/query/testing";
import {
  MAX_HISTORY_PAGES,
  answerApprovalMutationOptions,
  conversationKeys,
  historyQueryOptions,
  interruptMutationOptions,
  isApprovalAnswer,
  pendingAnswerFilters,
  pendingDecision,
  steerMutationOptions,
  withdrawPromptMutationOptions,
} from "./conversation-queries";

const REF = { machine: "m1", id: "c1" };

/** A promise that a test settles later. */
function deferred<T>(): { readonly promise: Promise<T>; readonly resolve: (value: T) => void } {
  let settle: ((value: T) => void) | undefined;
  const promise = new Promise<T>((done) => {
    settle = done;
  });
  return { promise, resolve: (value) => settle?.(value) };
}

// A history of 20 pages: page n holds event n, and its cursor is `p<n>`. The newest
// page is 0; page 19 is the oldest and has no next cursor.
function history(): FakeCaller {
  return new FakeCaller({
    "conversation.history": (params: ConversationHistory): ConversationHistoryResult => {
      const page =
        params.cursor === undefined || params.cursor === null ? 0 : Number(params.cursor.slice(1));
      const result: ConversationHistoryResult = {
        events: [
          {
            seq: 100 - page,
            at: "2026-10-26T12:00:00Z",
            conversation_id: "c1",
            event: { kind: "conversation_deleted", origin: "shell" },
          },
        ],
      };
      if (page < 19) {
        result.next_cursor = `p${page + 1}`;
      }
      return result;
    },
  });
}

function pageNumbers(data: InfiniteData<HistoryPage> | undefined): number[] {
  return (data?.pages ?? []).map((page) => 100 - (page.events[0]?.seq ?? 100));
}

async function open(client: QueryClient, caller: FakeCaller, start: string | null) {
  const observer = new InfiniteQueryObserver(client, historyQueryOptions(caller, REF, start));
  const stop = observer.subscribe(() => undefined);
  await vi.advanceTimersByTimeAsync(0);
  return { observer, stop };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("conversationKeys", () => {
  it("puts every key of a conversation under its machine and conversation", () => {
    const root = [...machineKey("m1"), "conversation", "c1"];
    expect(conversationKeys.conversation(REF)).toEqual(root);
    for (const key of [
      conversationKeys.history(REF, null),
      conversationKeys.approvalAnswers(REF),
      conversationKeys.steer(REF),
      conversationKeys.interrupt(REF),
      conversationKeys.withdraw(REF),
    ]) {
      expect(key.slice(0, root.length)).toEqual(root);
    }
    expect(conversationKeys.history(REF, "s:41")).not.toEqual(conversationKeys.history(REF, null));
  });
});

describe("historyQueryOptions", () => {
  it("starts at the cursor of the snapshot and goes to older pages", async () => {
    const caller = history();
    const { observer, stop } = await open(createQueryClient(), caller, "p3");
    expect(caller.calls[0]?.params).toEqual({ conversation_id: "c1", cursor: "p3" });
    expect(caller.calls[0]?.signal).toBeInstanceOf(AbortSignal);
    await observer.fetchNextPage();
    expect(pageNumbers(observer.getCurrentResult().data)).toEqual([3, 4]);
    expect(observer.getCurrentResult().hasPreviousPage).toBe(false);
    stop();
  });

  it("asks for the newest page without a cursor", async () => {
    const caller = history();
    const { stop } = await open(createQueryClient(), caller, null);
    expect(caller.calls[0]?.params).toEqual({ conversation_id: "c1", cursor: null });
    stop();
  });

  it("ends at the oldest page", async () => {
    const caller = history();
    const { observer, stop } = await open(createQueryClient(), caller, "p18");
    expect(observer.getCurrentResult().hasNextPage).toBe(true);
    await observer.fetchNextPage();
    expect(pageNumbers(observer.getCurrentResult().data)).toEqual([18, 19]);
    expect(observer.getCurrentResult().hasNextPage).toBe(false);
    stop();
  });

  it("keeps at most MAX_HISTORY_PAGES and gets the dropped newer pages back", async () => {
    const caller = history();
    const { observer, stop } = await open(createQueryClient(), caller, null);
    for (let page = 1; page < MAX_HISTORY_PAGES + 3; page += 1) {
      await observer.fetchNextPage();
    }
    expect(pageNumbers(observer.getCurrentResult().data)).toEqual(
      Array.from({ length: MAX_HISTORY_PAGES }, (_, i) => i + 3),
    );
    expect(observer.getCurrentResult().hasPreviousPage).toBe(true);

    await observer.fetchPreviousPage();
    expect(caller.calls.at(-1)?.params).toEqual({ conversation_id: "c1", cursor: "p2" });
    await observer.fetchPreviousPage();
    await observer.fetchPreviousPage();
    expect(caller.calls.at(-1)?.params).toEqual({ conversation_id: "c1", cursor: null });
    expect(pageNumbers(observer.getCurrentResult().data)).toEqual(
      Array.from({ length: MAX_HISTORY_PAGES }, (_, i) => i),
    );
    expect(observer.getCurrentResult().hasPreviousPage).toBe(false);
    expect(observer.getCurrentResult().hasNextPage).toBe(true);
    stop();
  });
});

describe("the mutations of a conversation", () => {
  it("answers an approval with the ids of the variables", async () => {
    const caller = new FakeCaller({ "approval.respond": () => ({ seq: 1 }) });
    const client = createQueryClient();
    const observer = new MutationObserver(client, answerApprovalMutationOptions(caller, REF));
    await observer.mutate({ callId: "k1", decision: "deny", commandId: "cmd-1" });
    expect(caller.calls).toEqual([
      {
        machine: "m1",
        method: "approval.respond",
        params: { call_id: "k1", command_id: "cmd-1", conversation_id: "c1", decision: "deny" },
        signal: undefined,
      },
    ]);
  });

  it("runs the writes of one conversation in order, and Stop at once", async () => {
    const answer = deferred<ApprovalRespondResult>();
    const caller = new FakeCaller({
      "approval.respond": () => answer.promise,
      "turn.steer": () => ({ seq: 2, turn_id: "t" }),
      "turn.interrupt": () => ({ seq: 3, turn_id: "t" }),
    });
    const client = createQueryClient();
    const answering = new MutationObserver(client, answerApprovalMutationOptions(caller, REF));
    const steering = new MutationObserver(client, steerMutationOptions(caller, REF));
    const stopping = new MutationObserver(client, interruptMutationOptions(caller, REF));
    const elsewhere = new MutationObserver(
      client,
      steerMutationOptions(caller, { machine: "m1", id: "c2" }),
    );

    const answered = answering.mutate({ callId: "k1", decision: "allow", commandId: "a" });
    const steered = steering.mutate({ text: "use the cache", commandId: "s" });
    await vi.advanceTimersByTimeAsync(0);
    expect(caller.calls.map((call) => call.method)).toEqual(["approval.respond"]);
    expect(steering.getCurrentResult().isPaused).toBe(true);

    const stopped = stopping.mutate({ commandId: "i" });
    const other = elsewhere.mutate({ text: "x", commandId: "o" });
    await vi.advanceTimersByTimeAsync(0);
    expect(caller.calls.map((call) => call.method)).toEqual([
      "approval.respond",
      "turn.interrupt",
      "turn.steer",
    ]);
    expect(caller.calls[2]?.params).toMatchObject({ conversation_id: "c2" });

    answer.resolve({ seq: 1 });
    await Promise.all([answered, steered, stopped, other]);
    expect(caller.calls.map((call) => call.method)).toEqual([
      "approval.respond",
      "turn.interrupt",
      "turn.steer",
      "turn.steer",
    ]);
  });

  it("queues a late steer and names the turn only when the screen knows it", async () => {
    const caller = new FakeCaller({ "turn.steer": () => ({ seq: 2, turn_id: "t" }) });
    const client = createQueryClient();
    const observer = new MutationObserver(client, steerMutationOptions(caller, REF));
    await observer.mutate({ text: "t", commandId: "a" });
    await observer.mutate({ text: "t", commandId: "b", turnId: "turn-1" });
    expect(caller.calls[0]?.params).toEqual({
      command_id: "a",
      conversation_id: "c1",
      text: "t",
      if_late: { kind: "queue" },
    });
    expect(Object.keys(caller.calls[0]?.params ?? {})).not.toContain("turn_id");
    expect(caller.calls[1]?.params).toMatchObject({ turn_id: "turn-1" });
  });

  it("stops the turn and withdraws a queued prompt", async () => {
    const caller = new FakeCaller({
      "turn.interrupt": () => ({ seq: 3, turn_id: "t" }),
      "prompt.withdraw": () => ({ withdrawn: { seq: 4, text: "x", turn_id: "turn-2" } }),
    });
    const client = createQueryClient();
    await new MutationObserver(client, interruptMutationOptions(caller, REF)).mutate({
      commandId: "i",
      turnId: "turn-1",
    });
    await new MutationObserver(client, withdrawPromptMutationOptions(caller, REF)).mutate({
      commandId: "w",
      turnId: "turn-2",
    });
    expect(caller.calls.map((call) => call.params)).toEqual([
      { command_id: "i", conversation_id: "c1", turn_id: "turn-1" },
      { command_id: "w", conversation_id: "c1", target: { kind: "turn", turn_id: "turn-2" } },
    ]);
  });

  it("marks the lists of the machine stale after an answer or a stop, even after a failure", async () => {
    const caller = new FakeCaller({ "turn.interrupt": () => ({ seq: 3, turn_id: "t" }) });
    const client = createQueryClient();
    client.setQueryData(conversationsKeys.recent("m1"), { conversations: [] });
    client.setQueryData(conversationsKeys.recent("m2"), { conversations: [] });
    const stale = (machine: string): boolean | undefined =>
      client.getQueryState(conversationsKeys.recent(machine))?.isInvalidated;

    await new MutationObserver(client, answerApprovalMutationOptions(caller, REF))
      .mutate({ callId: "k", decision: "allow", commandId: "a" })
      .catch(() => undefined);
    expect(stale("m1")).toBe(true);
    expect(stale("m2")).toBe(false);

    client.setQueryData(conversationsKeys.recent("m1"), { conversations: [] });
    await new MutationObserver(client, interruptMutationOptions(caller, REF)).mutate({
      commandId: "i",
    });
    expect(stale("m1")).toBe(true);
  });

  it("tells which answer is on its way", async () => {
    const answer = deferred<ApprovalRespondResult>();
    const caller = new FakeCaller({ "approval.respond": () => answer.promise });
    const client = createQueryClient();
    const observer = new MutationObserver(client, answerApprovalMutationOptions(caller, REF));
    const sent = observer.mutate({ callId: "k1", decision: "deny", commandId: "a" });
    await vi.advanceTimersByTimeAsync(0);
    const pending = client
      .getMutationCache()
      .findAll(pendingAnswerFilters(REF))
      .map((mutation) => mutation.state.variables);
    expect(pendingDecision(pending, "k1")).toBe("deny");
    expect(pendingDecision(pending, "k2")).toBeUndefined();
    expect(
      client.getMutationCache().findAll(pendingAnswerFilters({ machine: "m1", id: "c2" })),
    ).toEqual([]);
    answer.resolve({ seq: 1 });
    await sent;
    expect(client.getMutationCache().findAll(pendingAnswerFilters(REF))).toEqual([]);
  });
});

describe("pendingDecision", () => {
  it("takes the newest answer of the call and skips other values", () => {
    const values: unknown[] = [
      { callId: "k", decision: "allow", commandId: "1" },
      undefined,
      "text",
      { callId: "k", decision: "allow_always", commandId: "2" },
      { callId: "k", decision: "deny", commandId: "3" },
    ];
    expect(pendingDecision(values, "k")).toBe("deny");
    expect(pendingDecision([], "k")).toBeUndefined();
  });

  it("knows the variables of an answer", () => {
    expect(isApprovalAnswer({ callId: "k", decision: "allow", commandId: "c" })).toBe(true);
    expect(isApprovalAnswer({ callId: "k", decision: "allow" })).toBe(false);
    expect(isApprovalAnswer(null)).toBe(false);
  });
});
