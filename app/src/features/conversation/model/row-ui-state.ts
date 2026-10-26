// The state of the rows that the user opens and closes: a thought, the full output of a
// tool, a full diff, the remaining hunks of a long diff. It is one store per
// conversation, keyed by row key, never `useState` in the row: the list recycles row
// components, so a row's own state would move to another row (plan section 2.2).

import { createStore, type Readable } from "@tanstack/store";

/** What a row can open. */
export type RowFlag =
  /** The thought, the tool output or the diff shows in full. */
  | "expanded"
  /** A long diff shows all its hunks, not the first three. */
  | "all_hunks";

/** The open flags of each row, by row key. A row that is not here has none. */
export type RowUiState = ReadonlyMap<string, ReadonlySet<RowFlag>>;

/** The row state of one conversation. */
export interface RowUiStore {
  readonly store: Readable<RowUiState>;
  /** Turns `flag` of the row with `key` on or off. */
  set(key: string, flag: RowFlag, on: boolean): void;
  /** Turns `flag` of the row with `key` to its other value. */
  toggle(key: string, flag: RowFlag): void;
}

const NONE: ReadonlySet<RowFlag> = new Set();

/** A new row state with every row closed. */
export function createRowUiStore(): RowUiStore {
  const store = createStore<RowUiState>(new Map());
  const set = (key: string, flag: RowFlag, on: boolean): void => {
    store.setState((state) => {
      const flags = state.get(key) ?? NONE;
      if (flags.has(flag) === on) {
        return state;
      }
      const nextFlags = new Set(flags);
      if (on) {
        nextFlags.add(flag);
      } else {
        nextFlags.delete(flag);
      }
      const next = new Map(state);
      if (nextFlags.size === 0) {
        next.delete(key);
      } else {
        next.set(key, nextFlags);
      }
      return next;
    });
  };
  return {
    store,
    set,
    toggle: (key, flag) => {
      set(key, flag, !(store.get().get(key)?.has(flag) ?? false));
    },
  };
}

/** A selector: is `flag` of the row with `key` on. */
export function rowFlag(key: string, flag: RowFlag): (state: RowUiState) => boolean {
  return (state) => state.get(key)?.has(flag) ?? false;
}
