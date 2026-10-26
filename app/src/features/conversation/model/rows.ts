// The rows of the transcript list, oldest first (plan section 2.7), in the order that the
// list shows them. A row names its part by key only; the row's
// component reads the part from the stream store with a selector, so a row whose part
// did not change never renders again.
//
// The builder reuses the unchanged prefix: it keeps the rows of each turn object in a
// WeakMap, so a streaming frame, which changes only the running turn, builds the rows of
// that one turn (audit 7.14). It keeps each row object by key, and returns the same list
// while the keys stay the same, so the list itself renders only when a row comes or goes.

import type { StreamState, TurnView } from "@/lib/stream/reducer";

/** The kinds of rows. */
export type RowKind =
  | "prompt"
  | "reasoning"
  | "message"
  | "tool"
  | "approval"
  | "steer"
  /** The running line of a turn that runs now. */
  | "status"
  | "end";

/** One row of the transcript. */
export type Row =
  | { readonly kind: "prompt"; readonly key: string; readonly turnId: string }
  | { readonly kind: "reasoning"; readonly key: string; readonly turnId: string }
  | {
      readonly kind: "message";
      readonly key: string;
      readonly turnId: string;
      readonly index: number;
    }
  | {
      readonly kind: "tool";
      readonly key: string;
      readonly turnId: string;
      readonly callId: string;
    }
  | {
      readonly kind: "approval";
      readonly key: string;
      readonly turnId: string;
      readonly callId: string;
    }
  | { readonly kind: "steer"; readonly key: string; readonly turnId: string; readonly seq: number }
  | { readonly kind: "status"; readonly key: string; readonly turnId: string }
  | { readonly kind: "end"; readonly key: string; readonly turnId: string };

/** A function from the stream state to its rows, oldest first, that keeps its results. */
export type RowBuilder = (state: StreamState) => readonly Row[];

const KINDS: Readonly<Record<string, RowKind>> = {
  p: "prompt",
  r: "reasoning",
  m: "message",
  t: "tool",
  a: "approval",
  s: "steer",
  w: "status",
  e: "end",
};

/** The kind of the row with `key`, for the list's item type. */
export function rowKindOf(key: string): RowKind | undefined {
  return KINDS[key.slice(0, key.indexOf(":"))];
}

/**
 * The space between two rows of the transcript. Most rows are 12 pt apart (design item
 * 8.3). The answer starts 16 pt under your prompt, and a new turn starts 20 pt under the
 * end of the turn before it, so that each turn reads as one group. The end line of a turn
 * sits 4 pt under the last row of its turn, as the terminal prints it under the answer.
 */
export type RowGap = "item" | "afterPrompt" | "turn" | "end";

/** True for a row that holds your text: a prompt, a steer or a prompt of the outbox. */
function isPromptKey(key: string): boolean {
  return key.startsWith("p:") || key.startsWith("s:") || key.startsWith("o:");
}

/** True for the first row of a turn: a prompt, or a prompt of the outbox. */
function startsTurn(key: string): boolean {
  return key.startsWith("p:") || key.startsWith("o:");
}

/** The space between the row `above` and the row `below`, by their keys. */
export function rowGap(above: string, below: string): RowGap {
  if (startsTurn(below)) {
    return "turn";
  }
  if (below.startsWith("e:")) {
    return "end";
  }
  return isPromptKey(above) ? "afterPrompt" : "item";
}

/** True when `a` and `b` hold the same row objects in the same order. */
export function sameRows(a: readonly Row[], b: readonly Row[]): boolean {
  return a === b || (a.length === b.length && a.every((row, i) => row === b[i]));
}

/** A new builder with its own memo of rows. Each stream store has one. */
export function createRowBuilder(): RowBuilder {
  const byTurn = new WeakMap<TurnView, readonly Row[]>();
  const lastByTurnId = new Map<string, readonly Row[]>();
  const rowsByKey = new Map<string, Row>();
  let last: { readonly lists: readonly (readonly Row[])[]; readonly rows: readonly Row[] } = {
    lists: [],
    rows: [],
  };

  const keep = (row: Row): Row => {
    const held = rowsByKey.get(row.key);
    if (held !== undefined) {
      return held;
    }
    rowsByKey.set(row.key, row);
    return row;
  };

  const rowsOf = (turn: TurnView): readonly Row[] => {
    const cached = byTurn.get(turn);
    if (cached !== undefined) {
      return cached;
    }
    const built = turnRows(turn).map(keep);
    const previous = lastByTurnId.get(turn.turnId);
    const rows = previous !== undefined && sameRows(previous, built) ? previous : built;
    byTurn.set(turn, rows);
    lastByTurnId.set(turn.turnId, rows);
    return rows;
  };

  return (state) => {
    const lists: (readonly Row[])[] = [];
    for (const turnId of state.order) {
      const turn = state.turns.get(turnId);
      if (turn !== undefined) {
        lists.push(rowsOf(turn));
      }
    }
    if (lists.length === last.lists.length && lists.every((list, i) => list === last.lists[i])) {
      return last.rows;
    }
    const rows = lists.flat();
    last = { lists, rows: sameRows(last.rows, rows) ? last.rows : rows };
    return last.rows;
  };
}

function turnRows(turn: TurnView): Row[] {
  const turnId = turn.turnId;
  const rows: Row[] = [];
  if (turn.prompt !== undefined) {
    rows.push({ kind: "prompt", key: `p:${turnId}`, turnId });
  }
  if (turn.reasoning.text.length > 0) {
    rows.push({ kind: "reasoning", key: `r:${turnId}`, turnId });
  }
  for (const part of turn.parts) {
    switch (part.kind) {
      case "message":
        if ((turn.messages.get(part.index)?.buffer.text.length ?? 0) > 0) {
          rows.push({
            kind: "message",
            key: `m:${turnId}:${part.index}`,
            turnId,
            index: part.index,
          });
        }
        break;
      case "tool":
        rows.push({ kind: "tool", key: `t:${turnId}:${part.callId}`, turnId, callId: part.callId });
        break;
      case "approval":
        rows.push({
          kind: "approval",
          key: `a:${turnId}:${part.callId}`,
          turnId,
          callId: part.callId,
        });
        break;
      case "steer":
        // A steer that an interrupt sent again shows as the prompt of its new turn.
        if (turn.steers.get(part.seq)?.resent !== true) {
          rows.push({ kind: "steer", key: `s:${turnId}:${part.seq}`, turnId, seq: part.seq });
        }
        break;
      default:
        break;
    }
  }
  if (turn.end !== undefined) {
    rows.push({ kind: "end", key: `e:${turnId}`, turnId });
  } else if (turn.started) {
    rows.push({ kind: "status", key: `w:${turnId}`, turnId });
  }
  return rows;
}
