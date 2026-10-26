// The event cache in SQLite. It names only the part of the database API that it needs,
// which expo-sqlite's `SQLiteDatabase` has (src/store/native/database.ts opens it), so
// the tests run it on Node's SQLite.

import { MAX_CACHED_EVENTS, type CachedEvent, type EventCache } from "./event-cache";

/** A value that SQLite binds to a `?`. */
export type SqlValue = string | number | null;

/** The part of expo-sqlite's `SQLiteDatabase` that the cache uses. */
export interface SqlDatabase {
  execAsync(source: string): Promise<void>;
  runAsync(source: string, ...params: SqlValue[]): Promise<unknown>;
  getAllAsync<T>(source: string, ...params: SqlValue[]): Promise<T[]>;
}

/** The schema version that `SqliteEventCache.open` writes as `user_version`. */
export const CACHE_SCHEMA_VERSION = 1;

/** Rows per INSERT, well under SQLite's limit of bound values. */
const INSERT_CHUNK = 200;

/** The event cache in a SQLite database. */
export class SqliteEventCache implements EventCache {
  private readonly db: SqlDatabase;
  private readonly limit: number;

  private constructor(db: SqlDatabase, limit: number) {
    this.db = db;
    this.limit = limit;
  }

  /**
   * Opens the cache in `db` and makes its table. A database of another schema version
   * is a cache of another app build: the cache drops it and starts empty.
   */
  static async open(db: SqlDatabase, limit = MAX_CACHED_EVENTS): Promise<SqliteEventCache> {
    const rows = await db.getAllAsync<{ user_version: number }>("PRAGMA user_version");
    const version = rows[0]?.user_version ?? 0;
    if (version !== CACHE_SCHEMA_VERSION) {
      await db.execAsync("DROP TABLE IF EXISTS events");
    }
    await db.execAsync(
      [
        "CREATE TABLE IF NOT EXISTS events (",
        "  conversation_id TEXT NOT NULL,",
        "  seq INTEGER NOT NULL,",
        "  json TEXT NOT NULL,",
        "  PRIMARY KEY (conversation_id, seq)",
        ") WITHOUT ROWID;",
        `PRAGMA user_version = ${CACHE_SCHEMA_VERSION};`,
      ].join("\n"),
    );
    return new SqliteEventCache(db, limit);
  }

  async load(conversationId: string): Promise<readonly CachedEvent[]> {
    const rows = await this.db.getAllAsync<{ seq: number; json: string }>(
      "SELECT seq, json FROM events WHERE conversation_id = ? ORDER BY seq",
      conversationId,
    );
    return rows.map((row) => ({ seq: row.seq, json: row.json }));
  }

  async append(conversationId: string, events: readonly CachedEvent[]): Promise<void> {
    for (let start = 0; start < events.length; start += INSERT_CHUNK) {
      const chunk = events.slice(start, start + INSERT_CHUNK);
      const values = chunk.map(() => "(?, ?, ?)").join(", ");
      const params = chunk.flatMap((event) => [conversationId, event.seq, event.json]);
      // oxlint-disable-next-line no-await-in-loop -- the inserts run in order on one database.
      await this.db.runAsync(
        `INSERT OR REPLACE INTO events (conversation_id, seq, json) VALUES ${values}`,
        ...params,
      );
    }
    await this.db.runAsync(
      [
        "DELETE FROM events WHERE conversation_id = ? AND seq < (",
        "  SELECT seq FROM events WHERE conversation_id = ?",
        "  ORDER BY seq DESC LIMIT 1 OFFSET ?",
        ")",
      ].join("\n"),
      conversationId,
      conversationId,
      this.limit - 1,
    );
  }

  async replace(conversationId: string, events: readonly CachedEvent[]): Promise<void> {
    await this.remove(conversationId);
    await this.append(conversationId, events);
  }

  async remove(conversationId: string): Promise<void> {
    await this.db.runAsync("DELETE FROM events WHERE conversation_id = ?", conversationId);
  }
}
