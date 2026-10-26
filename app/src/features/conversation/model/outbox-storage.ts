// The outbox on the disk: a table `outbox` in the app's SQLite database, next to the
// event cache (src/lib/native/database.ts opens it). Each row holds the JSON of one entry.
// The load checks each row with the schema and deletes a row that fails it, so a broken
// row cannot stop the outbox.
//
// The table does not use the `user_version` of the event cache: a new cache schema drops
// the cached events, but never an unsent prompt.

import type { SqlDatabase } from "@/lib/store/sqlite-cache";
import type { CommandId, DaemonId } from "@/lib/protocol/generated";
import type { OutboxEntry, OutboxStorage } from "./outbox";
import { outboxEntrySchema } from "./outbox-schemas";

/** Reads one row, or undefined when it is not a valid entry. */
function parseRow(json: string): OutboxEntry | undefined {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    return undefined;
  }
  const parsed = outboxEntrySchema.safeParse(value);
  return parsed.success ? parsed.data : undefined;
}

/** The outbox in memory, for tests. It keeps JSON as SQLite does. */
export class MemoryOutboxStorage implements OutboxStorage {
  readonly rows = new Map<CommandId, { readonly machine: DaemonId; readonly json: string }>();

  load(): Promise<readonly OutboxEntry[]> {
    const entries: OutboxEntry[] = [];
    for (const [commandId, row] of this.rows) {
      const entry = parseRow(row.json);
      if (entry === undefined) {
        this.rows.delete(commandId);
      } else {
        entries.push(entry);
      }
    }
    entries.sort((a, b) => a.order - b.order);
    return Promise.resolve(entries);
  }

  put(entry: OutboxEntry): Promise<void> {
    this.rows.set(entry.commandId, { machine: entry.machine, json: JSON.stringify(entry) });
    return Promise.resolve();
  }

  remove(commandId: CommandId): Promise<void> {
    this.rows.delete(commandId);
    return Promise.resolve();
  }

  removeMachine(machine: DaemonId): Promise<void> {
    for (const [commandId, row] of this.rows) {
      if (row.machine === machine) {
        this.rows.delete(commandId);
      }
    }
    return Promise.resolve();
  }
}

/** The outbox in a SQLite database. */
export class SqliteOutboxStorage implements OutboxStorage {
  private readonly db: SqlDatabase;

  private constructor(db: SqlDatabase) {
    this.db = db;
  }

  /** Opens the outbox in `db` and makes its table. */
  static async open(db: SqlDatabase): Promise<SqliteOutboxStorage> {
    await db.execAsync(
      [
        "CREATE TABLE IF NOT EXISTS outbox (",
        "  command_id TEXT NOT NULL PRIMARY KEY,",
        "  machine TEXT NOT NULL,",
        "  ord REAL NOT NULL,",
        "  json TEXT NOT NULL",
        ") WITHOUT ROWID;",
      ].join("\n"),
    );
    return new SqliteOutboxStorage(db);
  }

  async load(): Promise<readonly OutboxEntry[]> {
    const rows = await this.db.getAllAsync<{ command_id: string; json: string }>(
      "SELECT command_id, json FROM outbox ORDER BY ord",
    );
    const entries: OutboxEntry[] = [];
    const broken: string[] = [];
    for (const row of rows) {
      const entry = parseRow(row.json);
      if (entry === undefined) {
        broken.push(row.command_id);
      } else {
        entries.push(entry);
      }
    }
    for (const commandId of broken) {
      // oxlint-disable-next-line no-await-in-loop -- the deletes run in order on one database.
      await this.remove(commandId);
    }
    return entries;
  }

  async put(entry: OutboxEntry): Promise<void> {
    await this.db.runAsync(
      "INSERT OR REPLACE INTO outbox (command_id, machine, ord, json) VALUES (?, ?, ?, ?)",
      entry.commandId,
      entry.machine,
      entry.order,
      JSON.stringify(entry),
    );
  }

  async remove(commandId: CommandId): Promise<void> {
    await this.db.runAsync("DELETE FROM outbox WHERE command_id = ?", commandId);
  }

  async removeMachine(machine: DaemonId): Promise<void> {
    await this.db.runAsync("DELETE FROM outbox WHERE machine = ?", machine);
  }
}
