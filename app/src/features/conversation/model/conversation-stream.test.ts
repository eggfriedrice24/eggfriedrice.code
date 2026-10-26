import { describe, expect, it } from "vitest";

import { decodeHistoryPage } from "@/lib/protocol/envelope";
import { CONVERSATION, TURN, item, rawEvent, rawItem } from "@/lib/protocol/testing";
import type { CachedEvent, ConversationCache } from "@/lib/store/event-cache";
import { envelope, manualFrames, messageCompleted, promptQueued } from "@/lib/stream/testing";
import type { StreamDeps } from "@/features/machines/model/stream-factory";
import { createConversationStream } from "./conversation-stream";

const REF = { machine: "019a9b1c-3d00-7a10-8b20-0000000000aa", id: CONVERSATION };

/** The cache of one conversation, in memory. */
class MemoryCache implements ConversationCache {
  rows = new Map<number, string>();

  load(): Promise<readonly CachedEvent[]> {
    const sorted = [...this.rows.entries()].sort((a, b) => a[0] - b[0]);
    return Promise.resolve(sorted.map(([seq, json]) => ({ seq, json })));
  }
  append(events: readonly CachedEvent[]): Promise<void> {
    for (const event of events) {
      this.rows.set(event.seq, event.json);
    }
    return Promise.resolve();
  }
  replace(events: readonly CachedEvent[]): Promise<void> {
    this.rows = new Map();
    return this.append(events);
  }
}

class BrokenCache implements ConversationCache {
  load(): Promise<never> {
    return Promise.reject(new Error("disk"));
  }
  append(): Promise<never> {
    return Promise.reject(new Error("disk"));
  }
  replace(): Promise<never> {
    return Promise.reject(new Error("disk"));
  }
}

/** A cache whose load answers only when the test says so. */
class SlowCache extends MemoryCache {
  private release: (() => void) | undefined;

  override load(): Promise<readonly CachedEvent[]> {
    const rows = super.load();
    return new Promise((resolve) => {
      this.release = () => resolve(rows);
    });
  }
  answer(): void {
    this.release?.();
  }
}

function draft(offset: number, delta: string, index = 0): unknown {
  return {
    kind: "draft",
    turn_id: TURN,
    after_seq: 1,
    draft: { kind: "text", index, offset, delta },
  };
}

function setup(deps: Partial<StreamDeps> = {}) {
  const frames = manualFrames();
  const stream = createConversationStream(REF, { schedule: frames.schedule, ...deps });
  const feed = (raw: unknown) => stream.item(item(raw), raw);
  return { frames, stream, feed };
}

describe("the conversation stream", () => {
  it("feeds the reducer and commits to the store once per frame", async () => {
    const { frames, stream, feed } = setup();
    await stream.hydrated;
    let commits = 0;
    stream.store.subscribe(() => commits++);
    feed(rawEvent("prompt_queued", 1));
    feed(rawEvent("turn_started", 2));
    feed(draft(0, "Hello"));
    feed(rawEvent("approval_requested", 3));
    expect(stream.store.get().lastSeq).toBe(0);
    expect(stream.latest().lastSeq).toBe(3);
    expect(stream.cursor()).toBe(3);
    frames.frame();
    expect(commits).toBe(1);
    const state = stream.store.get();
    expect(state.turns.get(TURN)?.prompt?.text).toBe("why is the disk full");
    expect(state.turns.get(TURN)?.messages.get(0)?.buffer.text).toBe("Hello");
    expect(state.loaded).toBe(true);
  });

  it("keeps its state for a duplicate, a draft that changes nothing and an unknown item", async () => {
    const { frames, stream, feed } = setup();
    await stream.hydrated;
    feed(rawEvent("turn_started", 2));
    feed(draft(0, "Hi"));
    frames.frame();
    const before = stream.latest();
    feed(rawEvent("turn_started", 2));
    feed(draft(0, "Hi"));
    stream.item({ kind: "other", wireKind: "presence" }, { kind: "presence" });
    expect(stream.latest()).toBe(before);
    expect(frames.pending()).toBe(0);
  });

  it("takes the summary of a snapshot and hands it on", async () => {
    const summaries: unknown[] = [];
    const { stream, feed } = setup({ onSummary: (summary) => summaries.push(summary.title) });
    await stream.hydrated;
    feed(rawItem("snapshot_live"));
    expect(stream.latest().summary?.status).toBe("running");
    expect(summaries).toEqual(["why is the disk full"]);
    expect(stream.cursor()).toBe(42);
  });

  it("tracks the state of its subscription", async () => {
    const { frames, stream } = setup();
    await stream.hydrated;
    stream.status({ state: "subscribing" });
    stream.status({ state: "live" });
    frames.frame();
    const live = stream.store.get();
    stream.status({ state: "live" });
    expect(stream.latest()).toBe(live);
    expect(live.link).toBe("live");
  });

  it("ignores items and asks for no frame after dispose", async () => {
    const { frames, stream, feed } = setup();
    await stream.hydrated;
    feed(rawEvent("turn_started", 1));
    stream.dispose();
    expect(frames.pending()).toBe(0);
    feed(rawEvent("turn_started", 2));
    expect(stream.cursor()).toBe(1);
  });
});

describe("the cache of the conversation stream", () => {
  it("writes events as efrd sent them and shows them at once on the next start", async () => {
    const cache = new MemoryCache();
    const first = setup({ cache });
    await first.stream.hydrated;
    first.feed(rawEvent("prompt_queued", 1));
    first.feed(rawEvent("assistant_message_completed", 2));
    first.feed(draft(0, "not cached", 1));
    await first.stream.settled();
    expect([...cache.rows.keys()]).toEqual([1, 2]);
    const parsed: unknown = JSON.parse(cache.rows.get(1) ?? "null");
    expect(parsed).toMatchObject({
      seq: 1,
      conversation_id: CONVERSATION,
      event: { kind: "prompt_queued", text: "why is the disk full", context: { pwd: "/var/log" } },
    });
    expect(parsed).not.toHaveProperty("kind");

    const second = setup({ cache });
    expect(second.stream.latest().loaded).toBe(false);
    await second.stream.hydrated;
    expect(second.frames.pending()).toBe(0);
    const state = second.stream.store.get();
    expect(state.loaded).toBe(true);
    expect(second.stream.cursor()).toBe(2);
    expect(state.turns.get(TURN)?.prompt?.text).toBe("why is the disk full");
    expect(state.turns.get(TURN)?.messages.get(0)?.final).toBe(true);
  });

  it("replaces the cached events with the events of a snapshot", async () => {
    const cache = new MemoryCache();
    await cache.append([{ seq: 5, json: "{}" }]);
    const { stream, feed } = setup({ cache });
    await stream.hydrated;
    feed(rawItem("snapshot"));
    await stream.settled();
    expect([...cache.rows.keys()]).toEqual([42]);
  });

  it("puts a cache that answers late under what the subscription gave", async () => {
    const cache = new SlowCache();
    await cache.append([
      { seq: 1, json: JSON.stringify(rawEvent("prompt_queued", 1)) },
      { seq: 2, json: JSON.stringify(rawEvent("turn_started", 2)) },
    ]);
    const { stream, feed } = setup({ cache });
    feed(rawEvent("assistant_message_completed", 5));
    cache.answer();
    await stream.hydrated;
    const turn = stream.store.get().turns.get(TURN);
    expect(turn?.prompt?.text).toBe("why is the disk full");
    expect(turn?.messages.get(0)?.final).toBe(true);
    expect(stream.cursor()).toBe(5);
  });

  it("puts a page of older history under the state and keeps it for the next start", async () => {
    const older = "019a9b1c-3d00-7a10-8b20-000000000003";
    const cache = new MemoryCache();
    const first = setup({ cache });
    await first.stream.hydrated;
    first.feed(rawEvent("prompt_queued", 5));
    first.stream.older([
      envelope(1, promptQueued(older, "the first question")),
      envelope(2, messageCompleted(older, 0, "The first answer.")),
    ]);
    first.stream.flush();
    expect(first.stream.store.get().order).toEqual([older, TURN]);
    expect(first.stream.cursor()).toBe(5);
    await first.stream.settled();
    expect([...cache.rows.keys()].sort((a, b) => a - b)).toEqual([1, 2, 5]);

    // After a cold start the older turns are still there.
    const second = setup({ cache });
    await second.stream.hydrated;
    expect(second.stream.store.get().order).toEqual([older, TURN]);
    expect(second.stream.store.get().turns.get(older)?.messages.get(0)?.buffer.text).toBe(
      "The first answer.",
    );
  });

  it("keeps the events of a history page in the cache as efrd sent them", async () => {
    const cache = new MemoryCache();
    const { stream } = setup({ cache });
    await stream.hydrated;
    const future = {
      seq: 3,
      at: "2026-10-26T12:00:00Z",
      conversation_id: CONVERSATION,
      event: { kind: "from_the_future", turn_id: TURN },
    };
    const decoded = decodeHistoryPage({ events: [future] });
    if (!decoded.ok) {
      throw new Error(decoded.reason);
    }
    expect(decoded.value.events[0]?.event).toEqual({ kind: "other", wireKind: "from_the_future" });
    stream.older(decoded.value.events, decoded.value.raw);
    await stream.settled();
    expect(cache.rows.get(3)).toBe(JSON.stringify(future));
  });

  it("skips cached rows that do not decode and goes on when the cache fails", async () => {
    const cache = new MemoryCache();
    await cache.append([
      { seq: 1, json: "not json" },
      {
        seq: 2,
        json: JSON.stringify({
          seq: 2,
          at: "2026-10-26T12:00:00Z",
          event: { kind: "from_the_future" },
        }),
      },
    ]);
    const good = setup({ cache });
    await good.stream.hydrated;
    expect(good.stream.cursor()).toBe(2);

    const errors: unknown[] = [];
    const broken = setup({ cache: new BrokenCache(), onCacheError: (error) => errors.push(error) });
    await broken.stream.hydrated;
    broken.feed(rawEvent("turn_started", 3));
    await broken.stream.settled();
    expect(errors).toHaveLength(2);
    expect(broken.stream.cursor()).toBe(3);
  });

  it("keeps a row state per conversation", () => {
    const { stream } = setup();
    stream.rowUi.toggle("t:t:c", "expanded");
    expect(stream.rowUi.store.get().get("t:t:c")?.has("expanded")).toBe(true);
  });
});
