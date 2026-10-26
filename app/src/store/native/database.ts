// Opens the event cache in expo-sqlite, and makes the command ids of the app's writes.

import { randomUUID } from "expo-crypto";
import { openDatabaseAsync } from "expo-sqlite";

import type { EventCache } from "../event_cache";
import { SqliteEventCache } from "../sqlite_cache";

/** The file of the event cache in the app's data. It holds no secret. */
export const CACHE_DATABASE = "efr-events.db";

/** Opens the event cache. */
export async function openEventCache(): Promise<EventCache> {
  const db = await openDatabaseAsync(CACHE_DATABASE);
  await db.execAsync("PRAGMA journal_mode = WAL;");
  return SqliteEventCache.open(db);
}

/** A new command id (a v4 UUID), which makes a write such as `approval.respond` idempotent. */
export function newCommandId(): string {
  return randomUUID();
}
