import { describe, expect, it } from "vitest";

import type { SubscribeItem, WatchItem } from "@/lib/protocol/wire";
import {
  Client,
  type ClientState,
  type SubscriptionSink,
  type SubscriptionStatus,
  type WatchSink,
} from "./client";
import {
  DAEMON_ID,
  DEVICE_ID,
  FakeClock,
  FakeKey,
  FakeSockets,
  completeHandshake,
  flush,
  remoteHello,
  type FakeSocket,
} from "./testing";

const ENDPOINT = { daemonId: DAEMON_ID, host: "100.64.0.1", port: 7419, deviceId: DEVICE_ID };
const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000000001";
const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";

function setup(random = 0) {
  const clock = new FakeClock();
  const sockets = new FakeSockets();
  const client = new Client({
    endpoint: ENDPOINT,
    key: new FakeKey(),
    sockets: sockets.factory,
    clock,
    random: () => random,
    client: "efr-app 0.0.0",
  });
  const states: ClientState["state"][] = [];
  client.onState((state) => states.push(state.state));
  return { clock, sockets, client, states };
}

async function started(random = 0) {
  const parts = setup(random);
  parts.client.start();
  await completeHandshake(parts.sockets.last);
  return parts;
}

class Sink implements SubscriptionSink {
  readonly items: SubscribeItem[] = [];
  readonly statuses: SubscriptionStatus["state"][] = [];
  seq: number | undefined;

  cursor(): number | undefined {
    return this.seq;
  }

  item(item: SubscribeItem): void {
    this.items.push(item);
    if (item.kind === "event") {
      this.seq = item.seq;
    }
  }

  status(status: SubscriptionStatus): void {
    this.statuses.push(status.state);
  }
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

/** efrd revokes the device: it refuses the proof. */
async function revoked(socket: FakeSocket): Promise<void> {
  socket.answer("hello", remoteHello());
  await flush();
  socket.refuse("device.prove", "unauthorized", "revoked");
}

/** Another daemon answers at the address. */
async function otherDaemon(socket: FakeSocket): Promise<void> {
  socket.answer("hello", remoteHello({ daemon_id: "019a9b1c-0000-7000-8000-000000000000" }));
  await flush();
}

/** A newer efrd speaks another protocol. */
async function newerEfrd(socket: FakeSocket): Promise<void> {
  socket.refuse("hello", "protocol_mismatch", "v2");
  await flush();
}

/** Lets `ms` pass while efrd answers each liveness probe, as its transport does. */
async function idle(clock: FakeClock, socket: FakeSocket, ms: number): Promise<void> {
  let answered = socket.requests("hello").length;
  for (let passed = 0; passed < ms; passed += 1000) {
    await clock.advance(1000);
    const probes = socket.requests("hello");
    if (probes.length > answered) {
      answered = probes.length;
      socket.refuse("hello", "conflict", "hello was already sent on this connection");
    }
  }
}

function subscribeParams(socket: FakeSocket): unknown[] {
  return socket.requests("conversation.subscribe").map((frame) => frame["params"]);
}

describe("the connection state", () => {
  it("connects, proves and is ready", async () => {
    const { client, states } = await started();
    expect(states).toEqual(["connecting", "ready"]);
    expect(client.state).toMatchObject({
      state: "ready",
      hello: { daemon_id: DAEMON_ID },
      scopes: ["read", "operate", "approve"],
    });
  });

  it("waits on the backoff ladder after a lost connection and connects again", async () => {
    const { client, clock, sockets, states } = await started();
    sockets.last.drop();
    await flush();
    // With random 0, each wait is half the ceiling: 1 s first.
    expect(client.state).toMatchObject({ state: "waiting", retryAt: clock.now() + 1000 });
    await clock.advance(999);
    expect(sockets.made).toHaveLength(1);
    await clock.advance(1);
    expect(sockets.made).toHaveLength(2);
    await completeHandshake(sockets.last);
    expect(states).toEqual(["connecting", "ready", "waiting", "connecting", "ready"]);
  });

  it("climbs the ladder while attempts fail, and resets it after 30 s up", async () => {
    const { client, clock, sockets } = setup();
    client.start();
    const waits: number[] = [];
    for (let i = 0; i < 4; i++) {
      sockets.last.drop();
      await flush();
      const state = client.state;
      waits.push(state.state === "waiting" ? state.retryAt - clock.now() : -1);
      await clock.advance(waits.at(-1) ?? 0);
    }
    expect(waits).toEqual([1000, 2000, 4000, 8000]);

    await completeHandshake(sockets.last);
    await idle(clock, sockets.last, 30_000);
    expect(client.state.state).toBe("ready");
    sockets.last.drop();
    await flush();
    const state = client.state;
    expect(state.state === "waiting" ? state.retryAt - clock.now() : -1).toBe(1000);
  });

  it("waits as long as efrd asks when it is starting", async () => {
    const { client, clock, sockets } = setup();
    client.start();
    sockets.last.open();
    await flush();
    sockets.last.refuse("hello", "busy", "efrd is starting", { retry_after_ms: 7000 });
    await flush();
    expect(client.state).toMatchObject({ state: "waiting", error: { kind: "starting" } });
    await clock.advance(6999);
    expect(sockets.made).toHaveLength(1);
    await clock.advance(1);
    expect(sockets.made).toHaveLength(2);
  });

  it("stops when the phone must pair again or the app must be updated", async () => {
    for (const [script, recovery] of [
      [revoked, "repair"],
      [otherDaemon, "repair"],
      [newerEfrd, "update"],
    ] as const) {
      const { client, clock, sockets } = setup();
      client.start();
      sockets.last.open();
      await flush();
      await script(sockets.last);
      await flush();
      const state = client.state;
      expect(state.state === "stopped" ? state.error.recovery : state.state).toBe(recovery);
      await clock.advance(3_600_000);
      expect(sockets.made).toHaveLength(1);
    }
  });

  it("starts again from stopped on start", async () => {
    const { client, sockets } = setup();
    client.start();
    sockets.last.open();
    await flush();
    sockets.last.refuse("hello", "protocol_mismatch", "v2");
    await flush();
    expect(client.state.state).toBe("stopped");
    client.start();
    expect(sockets.made).toHaveLength(2);
  });

  it("stops on stop and does not connect again", async () => {
    const { client, clock, sockets, states } = await started();
    client.stop();
    expect(sockets.last.closedWith).toBe(1000);
    await clock.advance(3_600_000);
    expect(sockets.made).toHaveLength(1);
    expect(states.at(-1)).toBe("off");
  });
});

describe("the foreground and the network path", () => {
  it("opens a new socket at once after 10 s or more in the background", async () => {
    const { client, clock, sockets } = await started();
    client.setForeground(false);
    await clock.advance(9_999);
    client.setForeground(true);
    expect(sockets.made).toHaveLength(1);

    client.setForeground(false);
    await clock.advance(10_000);
    client.setForeground(true);
    expect(sockets.made).toHaveLength(2);
    expect(sockets.made[0]?.closedWith).toBe(1000);
    expect(client.state.state).toBe("connecting");
    await completeHandshake(sockets.last);
    expect(client.state.state).toBe("ready");
  });

  it("connects at once when it comes back while waiting", async () => {
    const { client, clock, sockets } = await started();
    sockets.last.drop();
    await flush();
    expect(client.state.state).toBe("waiting");
    client.setForeground(false);
    await clock.advance(10);
    client.setForeground(true);
    expect(sockets.made).toHaveLength(2);
  });

  it("opens a new socket when the network path changes", async () => {
    const { client, sockets } = await started();
    client.networkChanged();
    expect(sockets.made).toHaveLength(2);
    expect(sockets.made[0]?.closedWith).toBe(1000);
    // The old socket's close does not count as a failure.
    expect(client.state.state).toBe("connecting");
  });
});

describe("subscriptions", () => {
  it("subscribes with drafts, passes decoded items, and resubscribes after the cursor", async () => {
    const { client, clock, sockets } = await started();
    const sink = new Sink();
    client.subscribe(CONVERSATION, sink);
    expect(subscribeParams(sockets.last)).toEqual([
      { conversation_id: CONVERSATION, drafts: true },
    ]);
    const id = sockets.last.lastId("conversation.subscribe");
    sockets.last.receive({ id, item: event(41) });
    sockets.last.receive({ id, item: event(42) });
    expect(sink.items.map((item) => (item.kind === "event" ? item.seq : 0))).toEqual([41, 42]);

    sockets.last.drop();
    await flush();
    await clock.advance(1000);
    await completeHandshake(sockets.last);
    expect(subscribeParams(sockets.last)).toEqual([
      { conversation_id: CONVERSATION, drafts: true, after_seq: 42 },
    ]);
    expect(sink.statuses).toEqual(["subscribing", "live", "waiting", "subscribing"]);
  });

  it("waits for a connection when it subscribes before one is ready", async () => {
    const { client, sockets } = setup();
    const sink = new Sink();
    client.subscribe(CONVERSATION, sink);
    expect(sink.statuses).toEqual(["waiting"]);
    client.start();
    await completeHandshake(sockets.last);
    expect(subscribeParams(sockets.last)).toHaveLength(1);
  });

  it("resubscribes after an overflow on its own backoff ladder", async () => {
    const { client, clock, sockets } = await started();
    const sink = new Sink();
    client.subscribe(CONVERSATION, sink);
    const socket = sockets.last;
    socket.receive({ id: socket.lastId("conversation.subscribe"), item: event(7) });
    socket.refuse("conversation.subscribe", "overflow", "behind", { last_seq: 7 });
    expect(sink.statuses.at(-1)).toBe("retrying");
    await clock.advance(999);
    expect(subscribeParams(socket)).toHaveLength(1);
    await clock.advance(1);
    expect(subscribeParams(socket).at(-1)).toEqual({
      conversation_id: CONVERSATION,
      drafts: true,
      after_seq: 7,
    });
    // A second overflow before any item waits twice as long.
    socket.refuse("conversation.subscribe", "overflow", "behind", { last_seq: 7 });
    await clock.advance(1999);
    expect(subscribeParams(socket)).toHaveLength(2);
    await clock.advance(1);
    expect(subscribeParams(socket)).toHaveLength(3);
    expect(client.state.state).toBe("ready");
  });

  it("stops after not_found, after the end, and on close", async () => {
    const { client, clock, sockets } = await started();
    const gone = new Sink();
    client.subscribe(CONVERSATION, gone);
    sockets.last.refuse("conversation.subscribe", "not_found", "no such conversation");
    const ended = new Sink();
    client.subscribe("019a9b1c-3d00-7a10-8b20-0000000000aa", ended);
    sockets.last.receive({ id: sockets.last.lastId("conversation.subscribe"), end: true });
    const closed = new Sink();
    const subscription = client.subscribe("019a9b1c-3d00-7a10-8b20-0000000000bb", closed);
    const closedId = sockets.last.lastId("conversation.subscribe");
    subscription.close();
    expect(sockets.last.sent.at(-1)).toEqual({ cancel: closedId });

    expect([gone.statuses.at(-1), ended.statuses.at(-1), closed.statuses.at(-1)]).toEqual([
      "failed",
      "ended",
      "closed",
    ]);
    sockets.last.drop();
    await flush();
    await clock.advance(1000);
    await completeHandshake(sockets.last);
    expect(subscribeParams(sockets.last)).toEqual([]);
  });

  it("resubscribes when an item is malformed", async () => {
    const { client, clock, sockets } = await started();
    const sink = new Sink();
    client.subscribe(CONVERSATION, sink);
    const id = sockets.last.lastId("conversation.subscribe");
    sockets.last.receive({ id, item: { kind: "event", seq: "x" } });
    expect(sockets.last.sent.at(-1)).toEqual({ cancel: id });
    expect(sink.statuses.at(-1)).toBe("retrying");
    await clock.advance(1000);
    expect(subscribeParams(sockets.last)).toHaveLength(2);
    expect(sink.items).toEqual([]);
  });
});

class Watcher implements WatchSink {
  readonly items: WatchItem[] = [];
  readonly statuses: SubscriptionStatus["state"][] = [];

  item(item: WatchItem): void {
    this.items.push(item);
  }

  status(status: SubscriptionStatus): void {
    this.statuses.push(status.state);
  }
}

/** A row of a watch, as efrd sends it. */
function row(id: string, status: string): unknown {
  return {
    summary: {
      id,
      title: "why is the disk full",
      status,
      created_at: "2026-10-26T11:00:00Z",
      updated_at: "2026-10-26T12:00:00Z",
      last_seq: 4,
    },
    route: { kind: "outside" },
    turns: 1,
  };
}

function watchParams(socket: FakeSocket): unknown[] {
  return socket.requests("conversations.watch").map((frame) => frame["params"]);
}

describe("the watch of the lists", () => {
  it("watches with its limit, passes decoded items, skips a kind it does not know, and watches again on a new connection", async () => {
    const { client, clock, sockets } = await started();
    const watcher = new Watcher();
    client.watch(20, watcher);
    expect(watchParams(sockets.last)).toEqual([{ limit: 20 }]);
    const id = sockets.last.lastId("conversations.watch");
    sockets.last.receive({
      id,
      item: { kind: "snapshot", conversations: [row(CONVERSATION, "idle")] },
    });
    sockets.last.receive({ id, item: { kind: "renamed", conversation_id: CONVERSATION } });
    sockets.last.receive({
      id,
      item: { kind: "changed", conversation: row(CONVERSATION, "paused") },
    });
    sockets.last.receive({
      id,
      item: { kind: "changed", conversation: row(CONVERSATION, "running") },
    });
    sockets.last.receive({ id, item: { kind: "removed", conversation_id: CONVERSATION } });
    expect(watcher.items.map((item) => item.kind)).toEqual([
      "snapshot",
      "other",
      "other",
      "changed",
      "removed",
    ]);
    expect(watcher.items[2]).toEqual({ kind: "other", wireKind: "changed", invalid: true });

    sockets.last.drop();
    await flush();
    await clock.advance(1000);
    await completeHandshake(sockets.last);
    expect(watchParams(sockets.last)).toEqual([{ limit: 20 }]);
    expect(watcher.statuses).toEqual(["subscribing", "live", "waiting", "subscribing"]);
  });

  it("watches again after an overflow and after a snapshot that is not valid, and stops on close", async () => {
    const { client, clock, sockets } = await started();
    const watcher = new Watcher();
    const watch = client.watch(20, watcher);
    const socket = sockets.last;
    socket.refuse("conversations.watch", "overflow", "behind", { last_seq: 9 });
    expect(watcher.statuses.at(-1)).toBe("retrying");
    await clock.advance(1000);
    expect(watchParams(socket)).toHaveLength(2);

    const id = socket.lastId("conversations.watch");
    socket.receive({ id, item: { kind: "snapshot", conversations: [{ summary: {} }] } });
    expect(socket.sent.at(-1)).toEqual({ cancel: id });
    expect(watcher.items).toEqual([]);
    await clock.advance(2000);
    expect(watchParams(socket)).toHaveLength(3);

    watch.close();
    expect(socket.sent.at(-1)).toEqual({ cancel: socket.lastId("conversations.watch") });
    expect(watcher.statuses.at(-1)).toBe("closed");
  });
});

describe("calls", () => {
  it("runs a call on the ready connection and refuses one while offline", async () => {
    const { client, sockets } = await started();
    const call = client.call("models.list", {});
    await flush();
    sockets.last.answer("models.list", { models: [] });
    expect(await call).toEqual({ models: [] });

    sockets.last.drop();
    await flush();
    await expect(client.call("models.list", {})).rejects.toMatchObject({ kind: "offline" });
  });
});
