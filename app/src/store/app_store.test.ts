import { describe, expect, it } from "vitest";

import { Client } from "../net/client";
import {
  DAEMON_ID,
  DEVICE_ID,
  FakeClock,
  FakeKey,
  FakeSockets,
  completeHandshake,
  flush,
  type FakeSocket,
} from "../net/testing";
import type { ConversationSummary } from "../protocol/generated";
import { AppStore } from "./app_store";
import { ConversationListStore } from "./conversations";
import { MemoryEventCache } from "./event_cache";
import { CONVERSATION, rawEvent, rawItem } from "./testing";

function summary(id: string, updatedAt: string): ConversationSummary {
  return {
    id,
    status: "idle",
    created_at: "2026-10-26T10:00:00Z",
    updated_at: updatedAt,
    last_seq: 1,
  };
}

const A = "019a9b1c-3d00-7a10-8b20-0000000000a1";
const B = "019a9b1c-3d00-7a10-8b20-0000000000b2";
const C = "019a9b1c-3d00-7a10-8b20-0000000000c3";

async function connected() {
  const clock = new FakeClock();
  const sockets = new FakeSockets();
  const client = new Client({
    endpoint: { daemonId: DAEMON_ID, host: "100.64.0.1", port: 7419, deviceId: DEVICE_ID },
    key: new FakeKey(),
    sockets: sockets.factory,
    clock,
    random: () => 0,
    client: "efr-app 0.0.0",
  });
  client.start();
  await completeHandshake(sockets.last);
  return { clock, sockets, client };
}

function answerList(socket: FakeSocket, conversations: unknown[], next?: string): void {
  socket.answer(
    "conversations.list",
    next === undefined ? { conversations } : { conversations, next_cursor: next },
  );
}

describe("the conversation list", () => {
  it("loads pages, the most recently updated first, and skips malformed summaries", async () => {
    const { client, sockets } = await connected();
    const list = new ConversationListStore();
    const refresh = list.refresh(client);
    expect(list.getState().loading).toBe(true);
    await flush();
    expect(sockets.last.requests("conversations.list")[0]?.["params"]).toEqual({ limit: 50 });
    answerList(
      sockets.last,
      [summary(A, "2026-10-26T12:00:05Z"), { id: 3 }, summary(B, "2026-10-26T12:00:05.5Z")],
      "page-2",
    );
    await refresh;
    expect(list.getState().conversations.map((held) => held.id)).toEqual([B, A]);
    expect(list.getState().nextCursor).toBe("page-2");

    const more = list.loadMore(client);
    await flush();
    expect(sockets.last.requests("conversations.list")[1]?.["params"]).toEqual({
      limit: 50,
      cursor: "page-2",
    });
    answerList(sockets.last, [summary(C, "2026-10-25T09:00:00Z")]);
    await more;
    expect(list.getState().conversations.map((held) => held.id)).toEqual([B, A, C]);
    expect(list.getState().nextCursor).toBeUndefined();
    await list.loadMore(client);
    expect(sockets.last.requests("conversations.list")).toHaveLength(2);
  });

  it("keeps the list and records the error when a load fails", async () => {
    const { client, sockets } = await connected();
    const list = new ConversationListStore();
    list.upsert(summary(A, "2026-10-26T12:00:00Z"));
    const refresh = list.refresh(client);
    await flush();
    sockets.last.refuse("conversations.list", "internal", "store error");
    await refresh;
    expect(list.getState()).toMatchObject({ loading: false, error: { name: "ServerError" } });
    expect(list.getState().conversations).toHaveLength(1);
  });

  it("puts in, updates and removes one summary", () => {
    const list = new ConversationListStore();
    list.upsert(summary(A, "2026-10-26T12:00:00Z"));
    list.upsert(summary(B, "2026-10-26T11:00:00Z"));
    list.upsert({ ...summary(B, "2026-10-26T13:00:00Z"), title: "newer" });
    expect(list.getState().conversations.map((held) => [held.id, held.title])).toEqual([
      [B, "newer"],
      [A, undefined],
    ]);
    list.remove(A);
    const state = list.getState();
    list.remove(A);
    expect(list.getState()).toBe(state);
    expect(state.conversations.map((held) => held.id)).toEqual([B]);
  });
});

describe("the app store", () => {
  it("loads the list on each new connection", async () => {
    const { client, sockets, clock } = await connected();
    const app = new AppStore({ client });
    await flush();
    expect(sockets.last.requests("conversations.list")).toHaveLength(1);
    sockets.last.drop();
    await flush();
    await clock.advance(1000);
    await completeHandshake(sockets.last);
    expect(sockets.last.requests("conversations.list")).toHaveLength(1);
    app.dispose();
  });

  it("shows the cache, then subscribes after it, and puts snapshot summaries in the list", async () => {
    const { client, sockets } = await connected();
    const cache = new MemoryEventCache();
    await cache.append(CONVERSATION, [
      { seq: 40, json: JSON.stringify(rawEvent("turn_started", 40)) },
    ]);
    const app = new AppStore({ client, cache });
    const held = app.hold(CONVERSATION);
    const again = app.hold(CONVERSATION);
    expect(again.store).toBe(held.store);
    await flush();
    expect(held.store.cursor()).toBe(40);
    const subscribes = sockets.last.requests("conversation.subscribe");
    expect(subscribes.map((frame) => frame["params"])).toEqual([
      { conversation_id: CONVERSATION, drafts: true, after_seq: 40 },
    ]);
    const id = sockets.last.lastId("conversation.subscribe");
    sockets.last.receive({ id, item: rawItem("snapshot") });
    expect(app.conversations.getState().conversations.map((listed) => listed.id)).toEqual([
      CONVERSATION,
    ]);

    held.release();
    held.release();
    expect(sockets.last.sent.filter((frame) => "cancel" in frame)).toEqual([]);
    again.release();
    expect(sockets.last.sent.filter((frame) => "cancel" in frame)).toEqual([{ cancel: id }]);
    expect(app.hold(CONVERSATION).store).not.toBe(held.store);
    app.dispose();
  });

  it("does not subscribe when the store is released before the cache loaded", async () => {
    const { client, sockets } = await connected();
    const app = new AppStore({ client, cache: new MemoryEventCache() });
    app.hold(CONVERSATION).release();
    await flush();
    expect(sockets.last.requests("conversation.subscribe")).toEqual([]);
    app.dispose();
  });
});
