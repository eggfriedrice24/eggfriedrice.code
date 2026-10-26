import { describe, expect, it } from "vitest";

import type { UnaryMethodName } from "@/lib/net/connection";
import type { MethodParams, MethodResults } from "@/lib/protocol/generated";
import { ApprovalRefused, ConversationStore, type Caller } from "./conversation";
import { MemoryEventCache, type EventCache } from "@/lib/store/event-cache";
import { CALL, CONVERSATION, TURN, item, rawEvent, rawItem } from "@/lib/protocol/testing";

function feed(store: ConversationStore, raw: unknown): void {
  store.item(item(raw), raw);
}

function draft(offset: number, delta: string, index = 0): unknown {
  return {
    kind: "draft",
    turn_id: TURN,
    after_seq: 1,
    draft: { kind: "text", index, offset, delta },
  };
}

class RecordingCaller implements Caller {
  readonly calls: unknown[] = [];

  call<M extends UnaryMethodName>(method: M, params: MethodParams[M]): Promise<MethodResults[M]> {
    this.calls.push({ method, params });
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- the result of approval.respond.
    return Promise.resolve({ seq: 99 } as never);
  }
}

class BrokenCache implements EventCache {
  load(): Promise<never> {
    return Promise.reject(new Error("disk"));
  }
  append(): Promise<never> {
    return Promise.reject(new Error("disk"));
  }
  replace(): Promise<never> {
    return Promise.reject(new Error("disk"));
  }
  remove(): Promise<never> {
    return Promise.reject(new Error("disk"));
  }
}

describe("the conversation store", () => {
  it("feeds events and drafts to the reducer, the approvals and the prompts", () => {
    const store = new ConversationStore(CONVERSATION);
    let changes = 0;
    store.subscribe(() => (changes += 1));
    feed(store, rawEvent("prompt_queued", 1));
    feed(store, rawEvent("turn_started", 2));
    feed(store, draft(0, "Hello"));
    feed(store, rawEvent("approval_requested", 3));
    const state = store.getState();
    expect(state.prompts.get(TURN)).toEqual({
      turnId: TURN,
      text: "why is the disk full",
      origin: "shell",
    });
    expect(state.stream.turns.get(TURN)?.messages.get(0)?.buffer.text).toBe("Hello");
    expect(state.approvals.byCall.get(CALL)?.status).toBe("pending");
    expect(state.loaded).toBe(true);
    expect(store.cursor()).toBe(3);
    expect(changes).toBe(4);
  });

  it("keeps its state's identity for a duplicate, a draft that changes nothing and an unknown item", () => {
    const store = new ConversationStore(CONVERSATION);
    feed(store, rawEvent("turn_started", 2));
    feed(store, draft(0, "Hi"));
    const before = store.getState();
    feed(store, rawEvent("turn_started", 2));
    feed(store, draft(0, "Hi"));
    store.item({ kind: "other", wireKind: "presence" }, { kind: "presence" });
    expect(store.getState()).toBe(before);
  });

  it("takes the summary of a snapshot and hands it on", () => {
    const summaries: unknown[] = [];
    const store = new ConversationStore(CONVERSATION, {
      onSummary: (summary) => summaries.push(summary.title),
    });
    feed(store, rawItem("snapshot_live"));
    const state = store.getState();
    expect(state.summary?.status).toBe("running");
    expect(summaries).toEqual(["why is the disk full"]);
    expect(store.cursor()).toBe(42);
    expect(state.stream.turns.get(TURN)?.messages.get(1)?.buffer.text).toBe(
      "The largest file is ~/日志/größe.log 🙂",
    );
  });

  it("tracks the state of its subscription", () => {
    const store = new ConversationStore(CONVERSATION);
    store.status({ state: "subscribing" });
    store.status({ state: "live" });
    const live = store.getState();
    store.status({ state: "live" });
    expect(store.getState()).toBe(live);
    expect(live.subscription).toBe("live");
  });
});

describe("the cache of the conversation store", () => {
  it("writes events as efrd sent them and shows them on the next start", async () => {
    const cache = new MemoryEventCache();
    const first = new ConversationStore(CONVERSATION, { cache });
    feed(first, rawEvent("prompt_queued", 1));
    feed(first, rawEvent("assistant_message_completed", 2));
    feed(first, draft(0, "not cached", 1));
    await first.settled();
    const rows = await cache.load(CONVERSATION);
    expect(rows.map((row) => row.seq)).toEqual([1, 2]);
    const parsed: unknown = JSON.parse(rows[0]?.json ?? "null");
    expect(parsed).toMatchObject({
      seq: 1,
      conversation_id: CONVERSATION,
      event: { kind: "prompt_queued", text: "why is the disk full", context: { pwd: "/var/log" } },
    });
    expect(parsed).not.toHaveProperty("kind");

    const second = new ConversationStore(CONVERSATION, { cache });
    expect(second.getState().loaded).toBe(false);
    await second.hydrate();
    const state = second.getState();
    expect(state.loaded).toBe(true);
    expect(second.cursor()).toBe(2);
    expect(state.prompts.get(TURN)?.text).toBe("why is the disk full");
    expect(state.stream.turns.get(TURN)?.messages.get(0)?.final).toBe(true);
  });

  it("hands each applied item on as JSON, the cached events as event items", async () => {
    const cache = new MemoryEventCache();
    const seen: unknown[] = [];
    const first = new ConversationStore(CONVERSATION, { cache, onItem: (raw) => seen.push(raw) });
    const event = rawEvent("prompt_queued", 1);
    feed(first, event);
    feed(first, event);
    feed(first, draft(0, "hi"));
    await first.settled();
    expect(seen).toEqual([event, draft(0, "hi")]);

    const again: unknown[] = [];
    const second = new ConversationStore(CONVERSATION, { cache, onItem: (raw) => again.push(raw) });
    await second.hydrate();
    expect(again).toEqual([event]);
  });

  it("replaces the cached events with the events of a snapshot", async () => {
    const cache = new MemoryEventCache();
    await cache.append(CONVERSATION, [{ seq: 5, json: "{}" }]);
    const store = new ConversationStore(CONVERSATION, { cache });
    feed(store, rawItem("snapshot"));
    await store.settled();
    expect((await cache.load(CONVERSATION)).map((row) => row.seq)).toEqual([42]);
  });

  it("skips cached rows that do not decode and goes on when the cache fails", async () => {
    const cache = new MemoryEventCache();
    await cache.append(CONVERSATION, [
      { seq: 1, json: "not json" },
      { seq: 2, json: JSON.stringify({ seq: 2, at: "t", event: { kind: "from_the_future" } }) },
    ]);
    const store = new ConversationStore(CONVERSATION, { cache });
    await store.hydrate();
    expect(store.cursor()).toBe(2);

    const errors: unknown[] = [];
    const broken = new ConversationStore(CONVERSATION, {
      cache: new BrokenCache(),
      onCacheError: (error) => errors.push(error),
    });
    await broken.hydrate();
    feed(broken, rawEvent("turn_started", 3));
    await broken.settled();
    expect(errors).toHaveLength(2);
    expect(broken.cursor()).toBe(3);
  });
});

describe("answers to approvals", () => {
  it("sends approval.respond for a pending question", async () => {
    const store = new ConversationStore(CONVERSATION);
    feed(store, rawEvent("approval_requested", 1));
    const caller = new RecordingCaller();
    await store.respond(caller, CALL, "allow", "cmd-1");
    expect(caller.calls).toEqual([
      {
        method: "approval.respond",
        params: {
          command_id: "cmd-1",
          conversation_id: CONVERSATION,
          call_id: CALL,
          decision: "allow",
        },
      },
    ]);
  });

  it("refuses local-only, answered and unknown questions, and allow_always", async () => {
    const caller = new RecordingCaller();
    const refusal = async (store: ConversationStore, decision: "allow" | "allow_always") => {
      try {
        await store.respond(caller, CALL, decision, "cmd");
      } catch (error) {
        return error instanceof ApprovalRefused ? error.reason : "other";
      }
      return "sent";
    };

    const local = new ConversationStore(CONVERSATION);
    feed(local, rawEvent("approval_requested_local_only", 1));
    const answered = new ConversationStore(CONVERSATION);
    feed(answered, rawEvent("approval_requested", 1));
    feed(answered, rawEvent("approval_resolved", 2));
    const always = new ConversationStore(CONVERSATION);
    feed(always, rawEvent("approval_requested", 1));

    expect([
      await refusal(local, "allow"),
      await refusal(answered, "allow"),
      await refusal(new ConversationStore(CONVERSATION), "allow"),
      await refusal(always, "allow_always"),
    ]).toEqual(["local_only", "not_pending", "unknown", "decision"]);
    expect(caller.calls).toEqual([]);
  });
});
