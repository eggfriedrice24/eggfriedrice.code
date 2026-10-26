// The part of Node's built-in SQLite (node:sqlite) that the tests of the SQLite event
// cache use. The app has no Node types (tsconfig `types` is empty), so this file names
// only what the tests call.

declare module "node:sqlite" {
  type SqlValue = string | number | null;

  export class StatementSync {
    run(...params: SqlValue[]): unknown;
    all(...params: SqlValue[]): unknown[];
  }

  export class DatabaseSync {
    constructor(path: string);
    exec(source: string): void;
    prepare(source: string): StatementSync;
    close(): void;
  }
}
