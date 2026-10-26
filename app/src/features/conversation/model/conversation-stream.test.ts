import { describe, expect, it } from "vitest";

import { CONVERSATION, TURN, item, rawEvent, rawItem } from "@/lib/protocol/testing";
import { manualFrames } from "@/lib/stream/testing";
import {
  type CachedEventRow,
  type ConversationCache,
  createConversationStream,
  type StreamDeps,
} from "./conversation-stream";

const REF = { machine: "019a9b1c-3d00-7a10-8b20-0000000000aa", conversation: CONVERSATION };

/** The cache of one conversation, in memory. */
class MemoryCache implements ConversationCache {
  rows = new Map<number, string>();

  load(): Promise<readonly CachedEventRow[]> {
    const sorted = [...this.rows.entries()].sort((a, b) => a[0] - b[0]);
    return Promise.resolve(sorted.map(([seq, json]) => ({ seq, json })));
  }
  append(events: readonly CachedEventRow[]): Promise<void> {
    for (const event of events) {
      this.rows.set(event.seq, event.json);
    }
    return Promise.resolve();
  }
  replace(events: readonly CachedEventRow[]): Promise<void> {
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

  override load(): Promise<readonly CachedEventRow[]> {
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
