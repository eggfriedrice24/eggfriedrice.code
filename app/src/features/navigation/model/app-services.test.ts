import { afterEach, describe, expect, it } from "vitest";

import { flush } from "@/lib/net/testing";
import type { EventEnvelope, SubscribeItem } from "@/lib/protocol/wire";
import { BLANK_ROOT, draftKey } from "@/features/conversation/model/drafts";
import {
  DESKTOP,
  LAPTOP,
  conversationId,
  lastSocket,
  testServices,
  type TestServices,
} from "../testing";
import { machineConnections } from "./machine-connections";
import { streamTitles } from "./stream-titles";

let parts: TestServices | undefined;

afterEach(() => {
  parts?.services.dispose();
  parts = undefined;
});

async function setup(): Promise<TestServices> {
  parts = await testServices();
  return parts;
}

const CONVERSATION = conversationId(1);

function socketOf(test: TestServices, host: string) {
  return lastSocket(test.sockets, `ws://${host}:7419/`);
}

function promptQueued(commandId: string, seq: number): EventEnvelope {
  return {
    seq,
    at: "2026-10-26T12:00:00Z",
    conversation_id: CONVERSATION,
    event: {
      kind: "prompt_queued",
      command_id: commandId,
      origin: "phone",
      text: "hello",
      turn_id: "019a9b1c-3d00-7a10-8b20-000000002001",
    },
  };
}

describe("the services of the app", () => {
  it("reads the paired machines and connects them all", async () => {
    const test = await setup();
    expect(test.services.runtime.registry.get().machines.map((machine) => machine.name)).toEqual([
      "desktop",
      "laptop",
    ]);
    expect(test.sockets.made.map((socket) => socket.url)).toEqual([
      "ws://100.64.0.1:7419/",
      "ws://100.64.0.2:7419/",
    ]);
  });

  it("sends a prompt of the outbox once its machine is ready", async () => {
    const test = await setup();
    test.services.outbox.enqueue({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: CONVERSATION },
      text: "hello",
    });
    await flush();
    expect(socketOf(test, "100.64.0.1").requests("prompt.send")).toHaveLength(0);
    await test.connect(DESKTOP);
    const sent = socketOf(test, "100.64.0.1").requests("prompt.send");
    expect(sent).toHaveLength(1);
    expect(sent[0]?.["params"]).toMatchObject({ conversation_id: CONVERSATION, text: "hello" });
    // The other machine sends nothing.
    expect(socketOf(test, "100.64.0.2").requests("prompt.send")).toHaveLength(0);
  });

  it("stops the drain while the machine is down, and sends again when it is back", async () => {
    const test = await setup();
    await test.connect(DESKTOP);
    socketOf(test, "100.64.0.1").drop();
    await flush();
    test.services.outbox.enqueue({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: CONVERSATION },
      text: "later",
    });
    await flush();
    expect(socketOf(test, "100.64.0.1").requests("prompt.send")).toHaveLength(0);
    // "Try again" opens a new socket at once.
    test.services.runtime.retry(DESKTOP);
    await test.connect(DESKTOP);
    expect(socketOf(test, "100.64.0.1").requests("prompt.send")).toHaveLength(1);
  });

  it("removes a sent prompt when its echo comes in the stream", async () => {
    const test = await setup();
    const entry = test.services.outbox.enqueue({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: CONVERSATION },
      text: "hello",
    });
    const hold = test.services.runtime.hold({ machine: DESKTOP, id: CONVERSATION }, "keep");
    if (hold === undefined) {
      throw new Error("the desktop is paired");
    }
    const item: SubscribeItem = { kind: "event", ...promptQueued(entry.commandId, 7) };
    hold.stream.item(item, item);
    expect(test.services.outbox.store.get().entries).toEqual([]);
    // The stream still has the event.
    expect(hold.stream.latest().lastSeq).toBe(7);
    hold.release();
  });

  it("acknowledges the echoes of a snapshot", async () => {
    const test = await setup();
    const entry = test.services.outbox.enqueue({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: CONVERSATION },
      text: "hello",
    });
    const hold = test.services.runtime.hold({ machine: DESKTOP, id: CONVERSATION }, "keep");
    if (hold === undefined) {
      throw new Error("the desktop is paired");
    }
    const snapshot: SubscribeItem = {
      kind: "snapshot",
      conversation: {
        id: CONVERSATION,
        status: "idle",
        created_at: "2026-10-26T09:00:00Z",
        updated_at: "2026-10-26T12:00:00Z",
        last_seq: 7,
        title: "Fan",
      },
      events: [promptQueued(entry.commandId, 7)],
      hwm: 7,
    };
    hold.stream.item(snapshot, snapshot);
    expect(test.services.outbox.store.get().entries).toEqual([]);
    hold.release();
  });

  it("forgets a machine with its drafts, its outbox and its place as the last one", async () => {
    const test = await setup();
    const { services } = test;
    services.preferences.setLastMachine(LAPTOP);
    services.drafts.set(draftKey(LAPTOP, BLANK_ROOT), "half a prompt");
    services.drafts.set(draftKey(DESKTOP, BLANK_ROOT), "keep me");
    services.outbox.enqueue({
      machine: LAPTOP,
      target: { kind: "conversation", conversationId: CONVERSATION },
      text: "hello",
    });
    await services.runtime.forget(LAPTOP);
    expect(services.preferences.store.get().lastMachine).toBeNull();
    expect(services.drafts.text(draftKey(LAPTOP, BLANK_ROOT))).toBe("");
    expect(services.drafts.text(draftKey(DESKTOP, BLANK_ROOT))).toBe("keep me");
    expect(services.outbox.store.get().entries).toEqual([]);
    expect(services.runtime.registry.get().machines.map((machine) => machine.name)).toEqual([
      "desktop",
    ]);
  });
});

describe("machineConnections", () => {
  it("follows each connection and each machine that comes or goes", async () => {
    const test = await setup();
    const store = machineConnections(test.services.runtime);
    expect(machineConnections(test.services.runtime)).toBe(store);
    expect(store.get().get(DESKTOP)?.view.kind).toBe("connecting");
    const seen: string[] = [];
    const subscription = store.subscribe((connections) => {
      seen.push(connections.get(DESKTOP)?.view.kind ?? "gone");
    });
    await test.connect(DESKTOP);
    expect(store.get().get(DESKTOP)?.view.kind).toBe("online");
    await test.services.runtime.forget(DESKTOP);
    expect(store.get().has(DESKTOP)).toBe(false);
    expect(seen).toContain("online");
    expect(seen.at(-1)).toBe("gone");
    subscription.unsubscribe();
  });
});

describe("streamTitles", () => {
  it("holds the stream while it is watched and reads the title of its snapshot", async () => {
    const test = await setup();
    const titles = streamTitles(test.services.runtime);
    const ref = { machine: DESKTOP, id: CONVERSATION };
    let changes = 0;
    const stop = titles.subscribe(ref, () => {
      changes += 1;
    });
    const second = titles.subscribe(ref, () => undefined);
    expect(titles.title(ref)).toBeUndefined();
    const hold = test.services.runtime.hold(ref, "keep");
    const snapshot: SubscribeItem = {
      kind: "snapshot",
      conversation: {
        id: CONVERSATION,
        status: "idle",
        created_at: "2026-10-26T09:00:00Z",
        updated_at: "2026-10-26T12:00:00Z",
        last_seq: 3,
        title: "Why is the fan loud",
      },
      events: [],
      hwm: 3,
    };
    hold?.stream.item(snapshot, snapshot);
    expect(titles.title(ref)).toBe("Why is the fan loud");
    expect(changes).toBeGreaterThan(0);
    stop();
    expect(titles.title(ref)).toBe("Why is the fan loud");
    second();
    expect(titles.title(ref)).toBeUndefined();
    hold?.release();
  });

  it("gives nothing for a machine that is not paired", async () => {
    const test = await setup();
    const titles = streamTitles(test.services.runtime);
    const stop = titles.subscribe(
      { machine: conversationId(9), id: CONVERSATION },
      () => undefined,
    );
    expect(titles.title({ machine: conversationId(9), id: CONVERSATION })).toBeUndefined();
    stop();
  });
});
