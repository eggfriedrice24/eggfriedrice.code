import { describe, expect, it } from "vitest";

import { ConnectionError } from "@/lib/net/errors";
import {
  FakeClock,
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
  type FakeSocket,
} from "@/lib/net/testing";
import { MemoryEventCache } from "@/lib/store/event-cache";
import type { DeviceKey } from "@/features/pairing/model/device-key";
import { createConversationStream } from "@/features/conversation/model/conversation-stream";
import type { Pairing } from "@/features/pairing/model/paired-device";
import type { MachineId } from "./machine-ref";
import { MachineSupervisor, STREAM_RETAIN_MS } from "./machine-supervisor";
import { DESKTOP, FakeStreams, machineRecord, pairedStores } from "./testing";

const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000000001";
const OTHER_CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000000002";
const TURN = "019a9b1c-3d00-7a10-8b20-000000000003";

const HELLO = remoteHello({ daemon_id: DESKTOP, version: "0.0.5" });

async function setup(options: { readonly withKey?: boolean } = {}) {
  const record = machineRecord(DESKTOP);
  const { keys } = await pairedStores([record]);
  const key: DeviceKey | null = options.withKey === false ? null : await keys.load(record.keyAlias);
  const clock = new FakeClock();
  const sockets = new FakeSockets();
  const streams = new FakeStreams();
  const cache = new MemoryEventCache();
  const ready: MachineId[] = [];
  const errors: unknown[] = [];
  const supervisor = new MachineSupervisor(
    { device: record, key },
    {
      sockets: sockets.factory,
      clock,
      random: () => 0,
      client: "efr-app test",
      streams: streams.factory,
      cache,
      schedule: (callback) => {
        callback();
        return () => {};
      },
      onError: (error) => errors.push(error),
      onReady: (machine) => ready.push(machine),
    },
  );
  return { record, keys, clock, sockets, streams, cache, ready, errors, supervisor };
}

async function connected(options: { readonly withKey?: boolean } = {}) {
  const parts = await setup(options);
  parts.supervisor.start();
  await completeHandshake(parts.sockets.last, HELLO);
  return parts;
}

function event(seq: number): unknown {
  return {
    kind: "event",
    seq,
    at: "2026-10-26T12:00:00Z",
    conversation_id: CONVERSATION,
    event: { kind: "turn_cancelled", turn_id: TURN },
  };
}

function subscribes(socket: FakeSocket): unknown[] {
  return socket.requests("conversation.subscribe").map((frame) => frame["params"]);
}

function cancelled(socket: FakeSocket, method: string): boolean {
  const id = socket.lastId(method);
  return socket.sent.some((frame) => frame["cancel"] === id);
}

describe("the connection atom", () => {
  it("follows the client, and says when the machine was seen", async () => {
    const { supervisor, sockets, clock, ready } = await setup();
    expect(supervisor.connection.get().view).toEqual({
      kind: "connecting",
      endpoint: "100.64.0.1:7419",
    });
    supervisor.start();
    await completeHandshake(sockets.last, HELLO);
    const up = supervisor.connection.get();
    expect(up.view).toEqual({ kind: "online", endpoint: "100.64.0.1:7419" });
    expect(up.seenAt).toBe(clock.now());
    expect(ready).toEqual([DESKTOP]);

    // Less than the silence that the liveness rule allows, so the drop ends the connection.
    await clock.advance(3_000);
    sockets.last.drop();
    await flush();
    const down = supervisor.connection.get();
    expect(down.view).toMatchObject({ kind: "unreachable", endpoint: "100.64.0.1:7419" });
    expect(down.seenAt).toBe(clock.now());

    await clock.advance(5_000);
    await completeHandshake(sockets.last, HELLO);
    expect(ready).toEqual([DESKTOP, DESKTOP]);
  });

  it("asks for a re-pair when the device key is gone, and runs no client", async () => {
    const { supervisor, sockets } = await setup({ withKey: false });
    supervisor.start();
    supervisor.retry();
    expect(sockets.made).toHaveLength(0);
    expect(supervisor.connection.get().view).toEqual({ kind: "repair_needed", reason: "other" });
    await expect(supervisor.call("conversations.recent", { limit: 20 })).rejects.toBeInstanceOf(
      ConnectionError,
    );
  });

  it("asks for a re-pair when efrd refused the device", async () => {
    const { supervisor, sockets } = await setup();
    supervisor.start();
    const socket = sockets.last;
    socket.open();
    await flush();
    socket.answer("hello", HELLO);
    await flush();
    socket.refuse("device.prove", "unauthorized", "revoked", { re_pair: true });
    await flush();
    expect(supervisor.connection.get().view).toEqual({
      kind: "repair_needed",
      reason: "node_changed",
    });
  });
});

describe("calls and status", () => {
  it("runs a method on the ready connection", async () => {
    const { supervisor, sockets } = await connected();
    const call = supervisor.call("conversations.recent", { limit: 20 });
    await flush();
    sockets.last.answer("conversations.recent", { conversations: [] });
    await expect(call).resolves.toEqual({ conversations: [] });
  });

  it("stops the wait when the signal aborts", async () => {
    const { supervisor } = await connected();
    const controller = new AbortController();
    const call = supervisor.call(
      "conversations.recent",
      { limit: 20 },
      {
        signal: controller.signal,
      },
    );
    controller.abort(new Error("the screen left"));
    await expect(call).rejects.toThrow("the screen left");
    const early = supervisor.call(
      "conversations.recent",
      { limit: 20 },
      {
        signal: controller.signal,
      },
    );
    await expect(early).rejects.toThrow("the screen left");
  });

  it("gives the status of efrd while connected, and offline else", async () => {
    const { supervisor, sockets, clock } = await setup();
    expect(() => supervisor.status()).toThrow(ConnectionError);
    supervisor.start();
    await completeHandshake(sockets.last, HELLO);
    expect(supervisor.status()).toEqual({
      version: "0.0.5",
      protocol: 1,
      scopes: ["read", "operate", "approve"],
      seenAt: clock.now(),
    });
  });
});

describe("with the stream of the conversation feature", () => {
  it("reads the machine's cache, resumes after it, and caches the new events", async () => {
    const record = machineRecord(DESKTOP);
    const { keys } = await pairedStores([record]);
    const key = await keys.load(record.keyAlias);
    const clock = new FakeClock();
    const sockets = new FakeSockets();
    const cache = new MemoryEventCache();
    await cache.append(CONVERSATION, [{ seq: 41, json: JSON.stringify(event(41)) }]);
    const errors: unknown[] = [];
    const supervisor = new MachineSupervisor(
      { device: record, key },
      {
        sockets: sockets.factory,
        clock,
        random: () => 0,
        client: "efr-app test",
        streams: createConversationStream,
        cache,
        schedule: (callback) => {
          callback();
          return () => {};
        },
        onError: (error) => errors.push(error),
        onReady: () => {},
      },
    );
    supervisor.start();
    await completeHandshake(sockets.last, HELLO);

    const hold = supervisor.hold(CONVERSATION, "follow");
    await hold.stream.hydrated;
    await flush();
    expect(hold.stream.ref).toEqual({ machine: DESKTOP, id: CONVERSATION });
    expect(subscribes(sockets.last)).toEqual([
      { conversation_id: CONVERSATION, drafts: true, after_seq: 41 },
    ]);

    const id = sockets.last.lastId("conversation.subscribe");
    sockets.last.receive({ id, item: event(42) });
    expect(hold.stream.cursor()).toBe(42);
    await hold.stream.settled();
    expect((await cache.load(CONVERSATION)).map((row) => row.seq)).toEqual([41, 42]);
    expect(errors).toEqual([]);
    supervisor.dispose();
  });
});

describe("the streams", () => {
  it("reads the cache first, then subscribes with after_seq from it", async () => {
    const { supervisor, sockets, streams } = await connected();
    streams.options = { cachedSeq: 41, slowCache: true };
    const hold = supervisor.hold(CONVERSATION, "follow");
    await flush();
    expect(subscribes(sockets.last)).toEqual([]);
    streams.last.finishCache();
    await flush();
    expect(subscribes(sockets.last)).toEqual([
      { conversation_id: CONVERSATION, drafts: true, after_seq: 41 },
    ]);
    expect(hold.stream).toBe(streams.last);
    expect(streams.last.ref).toEqual({ machine: DESKTOP, id: CONVERSATION });

    const id = sockets.last.lastId("conversation.subscribe");
    sockets.last.receive({ id, item: event(42) });
    expect(streams.last.items).toMatchObject([{ kind: "event", seq: 42 }]);
  });

  it("subscribes once for many follow holds, and stops with the last one", async () => {
    const { supervisor, sockets, streams } = await connected();
    const first = supervisor.hold(CONVERSATION, "follow");
    const second = supervisor.hold(CONVERSATION, "follow");
    await flush();
    expect(streams.made).toHaveLength(1);
    expect(subscribes(sockets.last)).toHaveLength(1);
    first.release();
    first.release();
    expect(cancelled(sockets.last, "conversation.subscribe")).toBe(false);
    second.release();
    expect(cancelled(sockets.last, "conversation.subscribe")).toBe(true);
    expect(streams.last.statuses.at(-1)).toBe("closed");
  });

  it("keeps a stream without a subscription for a keep hold", async () => {
    const { supervisor, sockets, streams } = await connected();
    const keep = supervisor.hold(CONVERSATION, "keep");
    await flush();
    expect(subscribes(sockets.last)).toEqual([]);
    const follow = supervisor.hold(CONVERSATION, "follow");
    await flush();
    expect(subscribes(sockets.last)).toHaveLength(1);
    follow.release();
    expect(cancelled(sockets.last, "conversation.subscribe")).toBe(true);
    // A blur releases the subscription; the screen in the stack keeps the stream.
    expect(streams.last.disposed).toBe(false);
    keep.release();
  });

  it("subscribes again with the stream's cursor when the screen has the focus again", async () => {
    const { supervisor, sockets } = await connected();
    const first = supervisor.hold(CONVERSATION, "follow");
    await flush();
    sockets.last.receive({ id: sockets.last.lastId("conversation.subscribe"), item: event(7) });
    first.release();
    const again = supervisor.hold(CONVERSATION, "follow");
    await flush();
    expect(subscribes(sockets.last)).toEqual([
      { conversation_id: CONVERSATION, drafts: true },
      { conversation_id: CONVERSATION, drafts: true, after_seq: 7 },
    ]);
    again.release();
  });

  it("keeps a stream for 5 minutes after the last hold, then drops it", async () => {
    const { supervisor, streams, clock } = await connected();
    const hold = supervisor.hold(CONVERSATION, "follow");
    const stream = hold.stream;
    hold.release();
    await clock.advance(STREAM_RETAIN_MS - 1);
    expect(stream).toMatchObject({ disposed: false });
    const back = supervisor.hold(CONVERSATION, "keep");
    expect(back.stream).toBe(stream);
    back.release();
    await clock.advance(STREAM_RETAIN_MS - 1);
    expect(stream).toMatchObject({ disposed: false });
    await clock.advance(1);
    expect(stream).toMatchObject({ disposed: true });
    expect(supervisor.streamCount).toBe(0);
    const fresh = supervisor.hold(CONVERSATION, "keep");
    expect(fresh.stream).not.toBe(stream);
    expect(streams.made).toHaveLength(2);
  });

  it("gives each conversation its own stream and its own cache", async () => {
    const { supervisor, streams, cache } = await connected();
    supervisor.hold(CONVERSATION, "keep");
    supervisor.hold(OTHER_CONVERSATION, "keep");
    expect(streams.made.map((stream) => stream.ref.id)).toEqual([CONVERSATION, OTHER_CONVERSATION]);
    await streams.last.deps.cache?.append([{ seq: 3, json: "{}" }]);
    expect(await cache.load(OTHER_CONVERSATION)).toEqual([{ seq: 3, json: "{}" }]);
    expect(await cache.load(CONVERSATION)).toEqual([]);
  });

  it("subscribes the followed streams when the connection comes up", async () => {
    const { supervisor, sockets } = await setup();
    supervisor.hold(CONVERSATION, "follow");
    supervisor.start();
    await completeHandshake(sockets.last, HELLO);
    expect(subscribes(sockets.last)).toHaveLength(1);
  });
});

describe("a re-pair and the end", () => {
  it("connects with the new device, and the streams and holds stay", async () => {
    const { supervisor, sockets, streams, record, keys, ready } = await connected();
    const hold = supervisor.hold(CONVERSATION, "follow");
    await flush();
    const old = sockets.last;
    const newKey = await keys.create("device-desktop-2");
    const repaired: Pairing = {
      device: {
        ...record,
        deviceId: "019a9b1c-3d00-7a10-8b20-0000000000f1",
        keyAlias: "device-desktop-2",
      },
      key: newKey,
    };
    supervisor.replace(repaired);
    expect(old.closedWith).toBeDefined();
    expect(sockets.made).toHaveLength(2);
    await completeHandshake(sockets.last, HELLO);
    expect(sockets.last.requests("device.prove")[0]?.["params"]).toMatchObject({
      device_id: "019a9b1c-3d00-7a10-8b20-0000000000f1",
    });
    expect(subscribes(sockets.last)).toHaveLength(1);
    expect(streams.made).toHaveLength(1);
    expect(hold.stream).toMatchObject({ disposed: false });
    expect(supervisor.device.keyAlias).toBe("device-desktop-2");
    expect(ready).toEqual([DESKTOP, DESKTOP]);
    expect(supervisor.connection.get().seenAt).toBeDefined();
  });

  it("closes the connection and drops every stream on dispose", async () => {
    const { supervisor, sockets, streams } = await connected();
    supervisor.hold(CONVERSATION, "follow");
    supervisor.hold(OTHER_CONVERSATION, "keep");
    await flush();
    supervisor.dispose();
    expect(sockets.last.closedWith).toBeDefined();
    expect(streams.made.map((stream) => stream.disposed)).toEqual([true, true]);
    expect(() => supervisor.hold(CONVERSATION, "keep")).toThrow("forgotten");
    supervisor.dispose();
  });
});
