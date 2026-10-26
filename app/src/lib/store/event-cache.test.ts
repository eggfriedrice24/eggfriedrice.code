import { describe, expect, it } from "vitest";

import {
  conversationCache,
  MemoryEventCaches,
  type CachedEvent,
  type EventCaches,
} from "./event-cache";
import { CACHE_SCHEMA_VERSION, SqliteEventCaches } from "./sqlite-cache";
import { NodeDatabase } from "./testing";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-000000000001";
const LAPTOP = "019a9b1c-3d00-7a10-8b20-000000000002";

function events(...seqs: number[]): CachedEvent[] {
  return seqs.map((seq) => ({ seq, json: `{"seq":${seq}}` }));
}

const KINDS: readonly [string, (limit: number) => Promise<EventCaches>][] = [
  ["memory", (limit) => Promise.resolve(new MemoryEventCaches(limit))],
  ["sqlite", (limit) => SqliteEventCaches.open(new NodeDatabase(), limit)],
];

describe.each(KINDS)("the %s event cache", (_name, make) => {
  it("loads what it holds, oldest first, per conversation", async () => {
    const cache = (await make(100)).forMachine(DESKTOP);
    await cache.append("a", events(3, 1));
    await cache.append("a", events(2));
    await cache.append("b", events(9));
    expect(await cache.load("a")).toEqual(events(1, 2, 3));
    expect(await cache.load("b")).toEqual(events(9));
    expect(await cache.load("c")).toEqual([]);
  });

  it("replaces an event with the same sequence number", async () => {
    const cache = (await make(100)).forMachine(DESKTOP);
    await cache.append("a", events(1));
    await cache.append("a", [{ seq: 1, json: "{}" }]);
    expect(await cache.load("a")).toEqual([{ seq: 1, json: "{}" }]);
  });

  it("keeps only the newest events past its limit", async () => {
    const cache = (await make(3)).forMachine(DESKTOP);
    await cache.append("a", events(1, 2, 3, 4, 5));
    expect(await cache.load("a")).toEqual(events(3, 4, 5));
    await cache.append("a", events(6));
    expect(await cache.load("a")).toEqual(events(4, 5, 6));
  });

  it("replaces and removes a conversation", async () => {
    const cache = (await make(100)).forMachine(DESKTOP);
    await cache.append("a", events(1, 2));
    await cache.append("b", events(1));
    await cache.replace("a", events(40, 41));
    expect(await cache.load("a")).toEqual(events(40, 41));
    await cache.remove("a");
    expect(await cache.load("a")).toEqual([]);
    expect(await cache.load("b")).toEqual(events(1));
  });

  it("gives one conversation of one machine to its stream", async () => {
    const caches = await make(100);
    const desktop = conversationCache(caches.forMachine(DESKTOP), "a");
    await desktop.append(events(2, 1));
    await conversationCache(caches.forMachine(LAPTOP), "a").append(events(7));
    expect(await desktop.load()).toEqual(events(1, 2));
    await desktop.replace(events(5));
    expect(await caches.forMachine(DESKTOP).load("a")).toEqual(events(5));
    expect(await caches.forMachine(LAPTOP).load("a")).toEqual(events(7));
  });

  it("writes a large batch in chunks", async () => {
    const cache = (await make(5000)).forMachine(DESKTOP);
    const many = events(...Array.from({ length: 1001 }, (_, i) => i + 1));
    await cache.append("a", many);
    expect(await cache.load("a")).toEqual(many);
  });

  it("keeps the conversations of two machines apart, with the same id", async () => {
    const caches = await make(3);
    const desktop = caches.forMachine(DESKTOP);
    const laptop = caches.forMachine(LAPTOP);
    await desktop.append("a", events(1, 2));
    await laptop.append("a", events(7, 8, 9));
    // The limit counts the events of one machine only.
    await laptop.append("a", events(10));
    expect(await desktop.load("a")).toEqual(events(1, 2));
    expect(await laptop.load("a")).toEqual(events(8, 9, 10));
    await laptop.replace("a", events(20));
    expect(await desktop.load("a")).toEqual(events(1, 2));
    await desktop.remove("a");
    expect(await laptop.load("a")).toEqual(events(20));
  });

  it("removes every event of a forgotten machine and keeps the others", async () => {
    const caches = await make(100);
    const desktop = caches.forMachine(DESKTOP);
    await desktop.append("a", events(1));
    await desktop.append("b", events(2));
    await caches.forMachine(LAPTOP).append("a", events(3));
    await caches.removeMachine(DESKTOP);
    // The cache object that a store still holds sees the machine empty too.
    expect(await desktop.load("a")).toEqual([]);
    expect(await caches.forMachine(DESKTOP).load("b")).toEqual([]);
    expect(await caches.forMachine(LAPTOP).load("a")).toEqual(events(3));
    // A machine that the cache never saw is no error.
    await caches.removeMachine("019a9b1c-3d00-7a10-8b20-0000000000ff");
  });
});

describe("the SQLite event cache", () => {
  it("keeps its events across opens of one schema version", async () => {
    const db = new NodeDatabase();
    const first = await SqliteEventCaches.open(db);
    await first.forMachine(DESKTOP).append("a", events(1));
    const second = await SqliteEventCaches.open(db);
    expect(await second.forMachine(DESKTOP).load("a")).toEqual(events(1));
    expect(db.db.prepare("PRAGMA user_version").all()).toEqual([
      { user_version: CACHE_SCHEMA_VERSION },
    ]);
  });

  it("drops the events of another schema version", async () => {
    const db = new NodeDatabase();
    const cache = await SqliteEventCaches.open(db);
    await cache.forMachine(DESKTOP).append("a", events(1));
    db.db.exec("PRAGMA user_version = 99");
    const reopened = await SqliteEventCaches.open(db);
    expect(await reopened.forMachine(DESKTOP).load("a")).toEqual([]);
  });

  it("drops the table of schema 1, which had no daemon id, and keeps other tables", async () => {
    expect(CACHE_SCHEMA_VERSION).toBe(2);
    const db = new NodeDatabase();
    db.db.exec(
      [
        "CREATE TABLE events (conversation_id TEXT NOT NULL, seq INTEGER NOT NULL,",
        "  json TEXT NOT NULL, PRIMARY KEY (conversation_id, seq)) WITHOUT ROWID;",
        "INSERT INTO events VALUES ('a', 1, '{}');",
        "CREATE TABLE outbox (command_id TEXT PRIMARY KEY);",
        "INSERT INTO outbox VALUES ('c');",
        "PRAGMA user_version = 1;",
      ].join("\n"),
    );
    const cache = await SqliteEventCaches.open(db);
    expect(await cache.forMachine(DESKTOP).load("a")).toEqual([]);
    await cache.forMachine(DESKTOP).append("a", events(5));
    expect(await cache.forMachine(DESKTOP).load("a")).toEqual(events(5));
    expect(db.db.prepare("SELECT command_id FROM outbox").all()).toEqual([{ command_id: "c" }]);
  });
});
