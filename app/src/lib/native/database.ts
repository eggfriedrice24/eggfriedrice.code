// Opens the app's SQLite database in expo-sqlite, and makes the command ids of the app's
// writes. The database holds the event cache (src/lib/store/sqlite-cache.ts) and the
// outbox of prompts (src/features/conversation/model/outbox-storage.ts). The runtime
// opens it once and gives it to both.

import { randomUUID } from "expo-crypto";
import { openDatabaseAsync } from "expo-sqlite";

import type { EventCaches } from "@/lib/store/event-cache";
import { SqliteEventCaches, type SqlDatabase } from "@/lib/store/sqlite-cache";

/** The file of the app's database. It holds no secret. */
export const CACHE_DATABASE = "efr-events.db";

/** Opens the app's database. Call it once; the result is shared. */
export async function openAppDatabase(): Promise<SqlDatabase> {
  const db = await openDatabaseAsync(CACHE_DATABASE);
  await db.execAsync("PRAGMA journal_mode = WAL;");
  return db;
}

/** Opens the event caches of all machines in the app's database. */
export function openEventCaches(db: SqlDatabase): Promise<EventCaches> {
  return SqliteEventCaches.open(db);
}

/** A new command id (a v4 UUID), which makes a write such as `prompt.send` idempotent. */
export function newCommandId(): string {
  return randomUUID();
}
