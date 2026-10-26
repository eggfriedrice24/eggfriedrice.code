// Fakes for the tests of the stores on the phone: expo-sqlite's async API over Node's
// SQLite, and a key-value store in memory with a switch that makes it fail. Only test
// files import this module.

import { DatabaseSync } from "node:sqlite";

import type { KeyValueStorage } from "./key-value";
import type { SqlDatabase, SqlValue } from "./sqlite-cache";

/** expo-sqlite's async API over Node's SQLite in memory. */
export class NodeDatabase implements SqlDatabase {
  readonly db = new DatabaseSync(":memory:");

  execAsync(source: string): Promise<void> {
    this.db.exec(source);
    return Promise.resolve();
  }

  runAsync(source: string, ...params: SqlValue[]): Promise<unknown> {
    return Promise.resolve(this.db.prepare(source).run(...params));
  }

  getAllAsync<T>(source: string, ...params: SqlValue[]): Promise<T[]> {
    // Node returns plain row objects; each caller reads the columns that it selected.
    // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- rows of the query.
    return Promise.resolve(this.db.prepare(source).all(...params) as T[]);
  }
}

/** A key-value store in memory. `failing` makes each call fail, as a full disk does. */
export class MemoryKeyValue implements KeyValueStorage {
  readonly items = new Map<string, string>();
  failing = false;
  /** The number of writes and removes, so that a test can count the saves. */
  writes = 0;

  getItemSync(key: string): string | null {
    if (this.failing) {
      throw new Error("the store failed");
    }
    return this.items.get(key) ?? null;
  }

  getItem(key: string): Promise<string | null> {
    if (this.failing) {
      return Promise.reject(new Error("the store failed"));
    }
    return Promise.resolve(this.items.get(key) ?? null);
  }

  setItem(key: string, value: string): Promise<void> {
    if (this.failing) {
      return Promise.reject(new Error("the store failed"));
    }
    this.writes += 1;
    this.items.set(key, value);
    return Promise.resolve();
  }

  removeItem(key: string): Promise<void> {
    if (this.failing) {
      return Promise.reject(new Error("the store failed"));
    }
    this.writes += 1;
    this.items.delete(key);
    return Promise.resolve();
  }
}
