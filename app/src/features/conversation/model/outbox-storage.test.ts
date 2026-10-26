import { describe, expect, it } from "vitest";

import { SqliteEventCaches } from "@/lib/store/sqlite-cache";
import { NodeDatabase } from "@/lib/store/testing";
import type { OutboxEntry, OutboxStorage } from "./outbox";
import { MemoryOutboxStorage, SqliteOutboxStorage } from "./outbox-storage";
import { CONVERSATION_A, DESKTOP, LAPTOP, idMaker } from "./outbox-testing";

const ids = idMaker();

function entry(machine: string, order: number, change: Partial<OutboxEntry> = {}): OutboxEntry {
  const base: OutboxEntry = {
    commandId: ids(),
    machine,
    target: { kind: "conversation", conversationId: CONVERSATION_A },
    text: `prompt ${order}`,
    createdAt: 1_700_000_000_000 + order,
    order,
    attempts: 0,
    state: "pending",
    retryAt: 0,
    error: null,
    sent: null,
  };
  return { ...base, ...change };
}

const KINDS: readonly [string, (db: NodeDatabase) => Promise<OutboxStorage>][] = [
  ["memory", () => Promise.resolve(new MemoryOutboxStorage())],
  ["sqlite", (db) => SqliteOutboxStorage.open(db)],
];

describe.each(KINDS)("the %s outbox storage", (_name, make) => {
  it("loads its entries by order, and replaces one by command id", async () => {
    const storage = await make(new NodeDatabase());
    const late = entry(DESKTOP, 30);
    const early = entry(DESKTOP, 10, {
      target: { kind: "new", localId: ids() },
      state: "sent",
      sent: { conversationId: CONVERSATION_A, turnId: ids(), seq: 4, queued: true },
    });
    await storage.put(late);
    await storage.put(early);
    expect(await storage.load()).toEqual([early, late]);
    const failed: OutboxEntry = {
      ...late,
      state: "failed",
      attempts: 2,
      error: { code: "invalid", message: "no" },
    };
    await storage.put(failed);
    expect(await storage.load()).toEqual([early, failed]);
  });

  it("removes one entry and every entry of a machine", async () => {
    const storage = await make(new NodeDatabase());
    const a = entry(DESKTOP, 1);
    const b = entry(DESKTOP, 2);
    const c = entry(LAPTOP, 3);
    for (const held of [a, b, c]) {
      await storage.put(held);
    }
    await storage.remove(a.commandId);
    expect(await storage.load()).toEqual([b, c]);
    await storage.removeMachine(DESKTOP);
    expect(await storage.load()).toEqual([c]);
  });

  it("skips and deletes a row that is not a valid entry", async () => {
    const storage = await make(new NodeDatabase());
    const good = entry(DESKTOP, 1);
    await storage.put(good);
    // Rows that the schema refuses, as from a broken write or another build.
    await storage.put({ ...entry(DESKTOP, 2), attempts: -1 });
    await storage.put({ ...entry(DESKTOP, 3), machine: "not a uuid" });
    expect(await storage.load()).toEqual([good]);
    expect(await storage.load()).toEqual([good]);
  });
});

describe("the SQLite outbox storage", () => {
  it("shares the database with the event cache and outlives a new cache schema", async () => {
    const db = new NodeDatabase();
    await SqliteEventCaches.open(db);
    const storage = await SqliteOutboxStorage.open(db);
    const held = entry(DESKTOP, 1);
    await storage.put(held);
    db.db.exec("PRAGMA user_version = 1");
    await SqliteEventCaches.open(db);
    const reopened = await SqliteOutboxStorage.open(db);
    expect(await reopened.load()).toEqual([held]);
  });

  it("deletes a row that is not JSON", async () => {
    const db = new NodeDatabase();
    const storage = await SqliteOutboxStorage.open(db);
    db.db.exec(
      `INSERT INTO outbox (command_id, machine, ord, json) VALUES ('x', '${DESKTOP}', 1, '{')`,
    );
    expect(await storage.load()).toEqual([]);
    expect(db.db.prepare("SELECT command_id FROM outbox").all()).toEqual([]);
  });
});
