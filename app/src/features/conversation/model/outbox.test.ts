import { describe, expect, it } from "vitest";

import type { PromptSendResult } from "@/lib/protocol/generated";
import {
  MAX_SEND_ATTEMPTS,
  drainPlan,
  entriesFor,
  resolveAlias,
  sendParams,
  type OutboxEntry,
  type OutboxSnapshot,
} from "./outbox";
import { MemoryOutboxStorage } from "./outbox-storage";
import { CONVERSATION_A, CONVERSATION_B, DESKTOP, LAPTOP, testOutbox } from "./outbox-testing";

const TO_A = { kind: "conversation", conversationId: CONVERSATION_A } as const;
const TO_B = { kind: "conversation", conversationId: CONVERSATION_B } as const;
const NEW_ID = "019a9b1c-3d00-7a10-8b20-0000000000c3";
const TURN = "019a9b1c-3d00-7a10-8b20-0000000000d4";

function result(conversationId: string, seq: number): PromptSendResult {
  return { conversation_id: conversationId, queued: false, seq, turn_id: TURN };
}

function texts(entries: readonly OutboxEntry[]): string[] {
  return entries.map((entry) => entry.text);
}

function due(snapshot: OutboxSnapshot, machine = DESKTOP, now = 0): string[] {
  return texts(drainPlan(snapshot, machine, now).due);
}

describe("the outbox", () => {
  it("shows an entry at once and writes it to the disk after", async () => {
    const storage = new MemoryOutboxStorage();
    const { outbox } = testOutbox(storage);
    const entry = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "hi" });
    expect(outbox.store.get().entries).toEqual([entry]);
    expect(entry).toMatchObject({ state: "pending", attempts: 0, retryAt: 0, sent: null });
    expect(storage.rows.size).toBe(0);
    await outbox.flush();
    expect(await storage.load()).toEqual([entry]);
  });

  it("gives each entry a rising order, also when the wall clock stops or goes back", () => {
    const { outbox, wall } = testOutbox();
    const first = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "1" });
    const second = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "2" });
    wall.time -= 60_000;
    const third = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "3" });
    expect(first.order).toBe(wall.time + 60_000);
    expect(second.order).toBeGreaterThan(first.order);
    expect(third.order).toBeGreaterThan(second.order);
    expect(third.createdAt).toBe(wall.time);
  });

  it("sends one entry per conversation at a time, in order", () => {
    const { outbox } = testOutbox();
    const a1 = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a1" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a2" });
    outbox.enqueue({ machine: DESKTOP, target: TO_B, text: "b1" });
    outbox.enqueue({ machine: LAPTOP, target: TO_A, text: "laptop" });
    expect(due(outbox.store.get())).toEqual(["a1", "b1"]);
    expect(due(outbox.store.get(), LAPTOP)).toEqual(["laptop"]);
    expect(outbox.begin(a1.commandId)).toBe(true);
    expect(outbox.begin(a1.commandId)).toBe(false);
    // a1 in flight holds its lane.
    expect(due(outbox.store.get())).toEqual(["b1"]);
    outbox.succeed(a1.commandId, result(CONVERSATION_A, 4));
    expect(due(outbox.store.get())).toEqual(["a2", "b1"]);
  });

  it("makes the params of each target", () => {
    const { outbox } = testOutbox();
    const known = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    const fresh = outbox.enqueue({ machine: DESKTOP, target: outbox.newConversation(), text: "n" });
    const snapshot = outbox.store.get();
    expect(sendParams(snapshot, known)).toEqual({
      command_id: known.commandId,
      text: "a",
      conversation_id: CONVERSATION_A,
    });
    expect(sendParams(snapshot, fresh)).toEqual({
      command_id: fresh.commandId,
      text: "n",
      new_conversation: true,
    });
  });

  it("resolves the local id of a new conversation once efrd made it", () => {
    const { outbox } = testOutbox();
    const target = outbox.newConversation();
    if (target.kind !== "new") {
      throw new Error("a new target");
    }
    const first = outbox.enqueue({ machine: DESKTOP, target, text: "first" });
    const second = outbox.enqueue({ machine: DESKTOP, target, text: "second" });
    // The second waits: a send without the alias would make a second conversation.
    expect(due(outbox.store.get())).toEqual(["first"]);
    outbox.begin(first.commandId);
    expect(due(outbox.store.get())).toEqual([]);
    expect(entriesFor(outbox.store.get(), DESKTOP, target.localId)).toHaveLength(2);

    outbox.succeed(first.commandId, result(NEW_ID, 1));
    const snapshot = outbox.store.get();
    expect(resolveAlias(snapshot, DESKTOP, target.localId)).toBe(NEW_ID);
    expect(resolveAlias(snapshot, LAPTOP, target.localId)).toBeUndefined();
    expect(sendParams(snapshot, second)).toEqual({
      command_id: second.commandId,
      text: "second",
      conversation_id: NEW_ID,
    });
    // A prompt for the local id now goes to the conversation.
    const late = outbox.enqueue({ machine: DESKTOP, target, text: "late" });
    expect(late.target).toEqual({ kind: "conversation", conversationId: NEW_ID });
    outbox.discard(late.commandId);
    // A prompt that the screen sends to the real id after setParams joins the same lane.
    outbox.enqueue({
      machine: DESKTOP,
      target: { kind: "conversation", conversationId: NEW_ID },
      text: "third",
    });
    expect(due(outbox.store.get())).toEqual(["second"]);
    expect(texts(entriesFor(outbox.store.get(), DESKTOP, target.localId))).toEqual([
      "first",
      "second",
      "third",
    ]);
    expect(texts(entriesFor(outbox.store.get(), DESKTOP, NEW_ID))).toEqual([
      "first",
      "second",
      "third",
    ]);
  });

  it("lets the next entry of a new conversation make it when the first one failed", () => {
    const { outbox } = testOutbox();
    const target = outbox.newConversation();
    const first = outbox.enqueue({ machine: DESKTOP, target, text: "first" });
    const second = outbox.enqueue({ machine: DESKTOP, target, text: "second" });
    outbox.begin(first.commandId);
    outbox.fail(first.commandId, { code: "invalid", message: "no" });
    expect(due(outbox.store.get())).toEqual(["second"]);
    expect(sendParams(outbox.store.get(), second)).toMatchObject({ new_conversation: true });
  });

  it("waits until retryAt and tells the drain when to wake", () => {
    const { outbox } = testOutbox();
    const a = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a2" });
    outbox.begin(a.commandId);
    outbox.retryLater(a.commandId, { code: "busy", message: "busy" }, 500);
    expect(drainPlan(outbox.store.get(), DESKTOP, 499)).toEqual({ due: [], wakeAt: 500 });
    expect(due(outbox.store.get(), DESKTOP, 500)).toEqual(["a"]);
    expect(outbox.store.get().entries[0]).toMatchObject({ attempts: 1, error: { code: "busy" } });
  });

  it("fails an entry after the last retryable attempt", () => {
    const { outbox } = testOutbox();
    const a = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    for (let attempt = 1; attempt <= MAX_SEND_ATTEMPTS; attempt++) {
      expect(outbox.begin(a.commandId)).toBe(true);
      outbox.retryLater(a.commandId, { code: "internal", message: "x" }, 0);
    }
    expect(outbox.store.get().entries[0]).toMatchObject({
      state: "failed",
      attempts: MAX_SEND_ATTEMPTS,
    });
  });

  it("makes a lost send pending again without an attempt", () => {
    const { outbox } = testOutbox();
    const a = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.begin(a.commandId);
    outbox.release(a.commandId, { code: "connection", message: "closed" });
    expect(outbox.store.get().entries[0]).toMatchObject({ state: "pending", attempts: 0 });
  });

  it("retries a failed entry with a new command id after the others", async () => {
    const storage = new MemoryOutboxStorage();
    const { outbox } = testOutbox(storage);
    const a = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    const b = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "b" });
    outbox.begin(a.commandId);
    outbox.fail(a.commandId, { code: "invalid", message: "bad" });
    expect(outbox.retry(b.commandId)).toBeUndefined();
    const again = outbox.retry(a.commandId);
    expect(again?.commandId).not.toBe(a.commandId);
    expect(again).toMatchObject({ state: "pending", attempts: 0, error: null, text: "a" });
    expect(texts(outbox.store.get().entries)).toEqual(["b", "a"]);
    await outbox.flush();
    expect(new Set(storage.rows.keys())).toEqual(new Set([b.commandId, again?.commandId]));
  });

  it("removes an entry on its echo, or when the stream passes its sequence number", () => {
    const { outbox } = testOutbox();
    const a = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    const b = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "b" });
    const c = outbox.enqueue({ machine: LAPTOP, target: TO_A, text: "c" });
    for (const [entry, seq] of [
      [a, 3],
      [b, 5],
      [c, 3],
    ] as const) {
      outbox.begin(entry.commandId);
      outbox.succeed(entry.commandId, result(CONVERSATION_A, seq));
    }
    expect(outbox.store.get().entries.map((entry) => entry.state)).toEqual([
      "sent",
      "sent",
      "sent",
    ]);
    outbox.settle(DESKTOP, CONVERSATION_A, 4);
    expect(texts(outbox.store.get().entries)).toEqual(["b", "c"]);
    outbox.acknowledge(b.commandId);
    expect(texts(outbox.store.get().entries)).toEqual(["c"]);
  });

  it("discards an entry that is not in flight", () => {
    const { outbox } = testOutbox();
    const a = outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    const b = outbox.enqueue({ machine: DESKTOP, target: TO_B, text: "b" });
    outbox.begin(a.commandId);
    outbox.discard(a.commandId);
    outbox.discard(b.commandId);
    expect(texts(outbox.store.get().entries)).toEqual(["a"]);
  });

  it("removes the entries and aliases of a forgotten machine, also on the disk", async () => {
    const storage = new MemoryOutboxStorage();
    const { outbox } = testOutbox(storage);
    const target = outbox.newConversation();
    if (target.kind !== "new") {
      throw new Error("a new target");
    }
    const fresh = outbox.enqueue({ machine: DESKTOP, target, text: "n" });
    outbox.begin(fresh.commandId);
    outbox.succeed(fresh.commandId, result(NEW_ID, 1));
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.enqueue({ machine: LAPTOP, target: TO_A, text: "laptop" });
    await outbox.removeMachine(DESKTOP);
    expect(texts(outbox.store.get().entries)).toEqual(["laptop"]);
    expect(resolveAlias(outbox.store.get(), DESKTOP, target.localId)).toBeUndefined();
    expect(texts(await storage.load())).toEqual(["laptop"]);
  });

  it("goes on in memory when the disk fails", async () => {
    const storage = new MemoryOutboxStorage();
    storage.put = () => Promise.reject(new Error("disk full"));
    const { outbox, errors } = testOutbox(storage);
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "b" });
    await outbox.flush();
    expect(texts(outbox.store.get().entries)).toEqual(["a", "b"]);
    expect(errors).toHaveLength(2);
  });
});

describe("the outbox after a restart", () => {
  it("sends an entry that was in flight again with its command id, at once", async () => {
    const storage = new MemoryOutboxStorage();
    const before = testOutbox(storage).outbox;
    const a = before.enqueue({ machine: DESKTOP, target: TO_A, text: "a" });
    const b = before.enqueue({ machine: DESKTOP, target: TO_B, text: "b" });
    before.begin(b.commandId);
    before.retryLater(b.commandId, { code: "busy", message: "busy" }, 9_999_999);
    before.begin(a.commandId);
    await before.flush();

    const { outbox } = testOutbox(storage, "11111111");
    await outbox.hydrate();
    const entries = outbox.store.get().entries;
    expect(entries.map((entry) => [entry.commandId, entry.state, entry.retryAt])).toEqual([
      [a.commandId, "pending", 0],
      [b.commandId, "pending", 0],
    ]);
    expect(due(outbox.store.get(), DESKTOP, 0)).toEqual(["a", "b"]);
  });

  it("keeps the aliases of sent entries", async () => {
    const storage = new MemoryOutboxStorage();
    const before = testOutbox(storage).outbox;
    const target = before.newConversation();
    if (target.kind !== "new") {
      throw new Error("a new target");
    }
    const first = before.enqueue({ machine: DESKTOP, target, text: "first" });
    before.enqueue({ machine: DESKTOP, target, text: "second" });
    before.begin(first.commandId);
    before.succeed(first.commandId, result(NEW_ID, 1));
    await before.flush();

    const { outbox } = testOutbox(storage, "11111111");
    await outbox.hydrate();
    expect(resolveAlias(outbox.store.get(), DESKTOP, target.localId)).toBe(NEW_ID);
    const second = outbox.store.get().entries[1];
    if (second === undefined) {
      throw new Error("two entries");
    }
    expect(sendParams(outbox.store.get(), second)).toMatchObject({ conversation_id: NEW_ID });
  });

  it("sends the rest of a new conversation to it after the echo of the first", async () => {
    const storage = new MemoryOutboxStorage();
    const before = testOutbox(storage).outbox;
    const target = before.newConversation();
    const first = before.enqueue({ machine: DESKTOP, target, text: "first" });
    before.enqueue({ machine: DESKTOP, target, text: "second" });
    before.begin(first.commandId);
    before.succeed(first.commandId, result(NEW_ID, 1));
    // The echo came, and then the app stopped before the second went out.
    before.acknowledge(first.commandId);
    await before.flush();

    const { outbox } = testOutbox(storage, "11111111");
    await outbox.hydrate();
    const second = outbox.store.get().entries[0];
    if (second === undefined) {
      throw new Error("one entry");
    }
    expect(second.target).toEqual({ kind: "conversation", conversationId: NEW_ID });
    expect(sendParams(outbox.store.get(), second)).toEqual({
      command_id: second.commandId,
      text: "second",
      conversation_id: NEW_ID,
    });
  });

  it("puts the entries of this launch after the loaded ones", async () => {
    const storage = new MemoryOutboxStorage();
    const before = testOutbox(storage);
    before.outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "old" });
    await before.outbox.flush();

    const after = testOutbox(storage, "11111111");
    // The clock of the phone went back before the app started again.
    after.wall.time -= 3_600_000;
    after.outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "new 1" });
    after.outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "new 2" });
    await after.outbox.hydrate();
    expect(texts(after.outbox.store.get().entries)).toEqual(["old", "new 1", "new 2"]);
    const later = after.outbox.enqueue({ machine: DESKTOP, target: TO_A, text: "new 3" });
    expect(texts(after.outbox.store.get().entries)).toEqual(["old", "new 1", "new 2", "new 3"]);
    await after.outbox.flush();
    expect(texts(await storage.load())).toEqual(["old", "new 1", "new 2", "new 3"]);
    expect(later.order).toBeGreaterThan(after.outbox.store.get().entries[2]?.order ?? 0);
  });

  it("starts empty when the load fails", async () => {
    const storage = new MemoryOutboxStorage();
    storage.load = () => Promise.reject(new Error("broken"));
    const { outbox, errors } = testOutbox(storage);
    await outbox.hydrate();
    expect(outbox.store.get().entries).toEqual([]);
    expect(errors).toHaveLength(1);
  });
});
