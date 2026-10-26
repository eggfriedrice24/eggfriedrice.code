import { DatabaseSync } from "node:sqlite";
import { describe, expect, it } from "vitest";

import { MemoryEventCache, type CachedEvent, type EventCache } from "./event_cache";
import {
  CACHE_SCHEMA_VERSION,
  SqliteEventCache,
  type SqlDatabase,
  type SqlValue,
} from "./sqlite_cache";

/** expo-sqlite's async API over Node's SQLite, for the tests. */
class NodeDatabase implements SqlDatabase {
  readonly db = new DatabaseSync(":memory:");

  execAsync(source: string): Promise<void> {
    this.db.exec(source);
    return Promise.resolve();
  }

  runAsync(source: string, ...params: SqlValue[]): Promise<unknown> {
    return Promise.resolve(this.db.prepare(source).run(...params));
  }

  getAllAsync<T>(source: string, ...params: SqlValue[]): Promise<T[]> {
    // Node returns plain row objects; each test reads the columns that it selected.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- rows of the query.
    return Promise.resolve(this.db.prepare(source).all(...params) as T[]);
  }
}

function events(...seqs: number[]): CachedEvent[] {
  return seqs.map((seq) => ({ seq, json: `{"seq":${seq}}` }));
}

const KINDS: readonly [string, (limit: number) => Promise<EventCache>][] = [
  ["memory", (limit) => Promise.resolve(new MemoryEventCache(limit))],
  ["sqlite", (limit) => SqliteEventCache.open(new NodeDatabase(), limit)],
];

describe.each(KINDS)("the %s event cache", (_name, make) => {
  it("loads what it holds, oldest first, per conversation", async () => {
    const cache = await make(100);
    await cache.append("a", events(3, 1));
    await cache.append("a", events(2));
    await cache.append("b", events(9));
    expect(await cache.load("a")).toEqual(events(1, 2, 3));
    expect(await cache.load("b")).toEqual(events(9));
    expect(await cache.load("c")).toEqual([]);
  });

  it("replaces an event with the same sequence number", async () => {
    const cache = await make(100);
    await cache.append("a", events(1));
    await cache.append("a", [{ seq: 1, json: "{}" }]);
    expect(await cache.load("a")).toEqual([{ seq: 1, json: "{}" }]);
  });

  it("keeps only the newest events past its limit", async () => {
    const cache = await make(3);
    await cache.append("a", events(1, 2, 3, 4, 5));
    expect(await cache.load("a")).toEqual(events(3, 4, 5));
    await cache.append("a", events(6));
    expect(await cache.load("a")).toEqual(events(4, 5, 6));
  });

  it("replaces and removes a conversation", async () => {
    const cache = await make(100);
    await cache.append("a", events(1, 2));
    await cache.append("b", events(1));
    await cache.replace("a", events(40, 41));
    expect(await cache.load("a")).toEqual(events(40, 41));
    await cache.remove("a");
    expect(await cache.load("a")).toEqual([]);
    expect(await cache.load("b")).toEqual(events(1));
  });

  it("writes a large batch in chunks", async () => {
    const cache = await make(5000);
    const many = events(...Array.from({ length: 1001 }, (_, i) => i + 1));
    await cache.append("a", many);
    expect(await cache.load("a")).toEqual(many);
  });
});

describe("the SQLite event cache", () => {
  it("keeps its events across opens of one schema version", async () => {
    const db = new NodeDatabase();
    const first = await SqliteEventCache.open(db);
    await first.append("a", events(1));
    const second = await SqliteEventCache.open(db);
    expect(await second.load("a")).toEqual(events(1));
    expect(db.db.prepare("PRAGMA user_version").all()).toEqual([
      { user_version: CACHE_SCHEMA_VERSION },
    ]);
  });

  it("drops the events of another schema version", async () => {
    const db = new NodeDatabase();
    const cache = await SqliteEventCache.open(db);
    await cache.append("a", events(1));
    db.db.exec("PRAGMA user_version = 99");
    const reopened = await SqliteEventCache.open(db);
    expect(await reopened.load("a")).toEqual([]);
  });
});
