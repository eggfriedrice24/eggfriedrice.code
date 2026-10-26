// The rows of the transcript list, newest first, as the inverted list takes them. A row
// names its part by key only; the row's component reads the part from the stores with a
// selector, so a row whose part did not change never renders again. The builder keeps
// each row object across calls and returns the same list while the keys stay the same,
// so the list itself renders only when a row comes or goes.

import type { StreamState, TurnView } from "../core/reducer";
import { memoSelector } from "./external_store";
import type { TranscriptDetails, TurnDetails } from "./details";

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
  /** What the running turn does now: thinking, writing a tool input, compacting. */
  | { readonly kind: "status"; readonly key: string; readonly turnId: string }
  | { readonly kind: "end"; readonly key: string; readonly turnId: string };

/** The inputs of the rows. */
export interface RowInput {
  readonly details: TranscriptDetails;
  readonly stream: StreamState;
}

/** A function from the stores' values to the rows, newest first, that keeps its results. */
export type RowBuilder = (input: RowInput) => readonly Row[];

/** A new builder with its own memo of rows. */
export function createRowBuilder(): RowBuilder {
  const cache = new Map<string, Row>();
  const keep = (row: Row): Row => {
    const held = cache.get(row.key);
    if (held !== undefined) {
      return held;
    }
    cache.set(row.key, row);
    return row;
  };
  const build = (input: RowInput): readonly Row[] => {
    const rows: Row[] = [];
    const seen = new Set<string>();
    const order = [...input.details.order];
    for (const turnId of input.stream.order) {
      if (!input.details.turns.has(turnId)) {
        order.push(turnId);
      }
    }
    for (const turnId of order) {
      const turn = input.details.turns.get(turnId);
      const stream = input.stream.turns.get(turnId);
      for (const row of turnRows(turnId, turn, stream)) {
        rows.push(keep(row));
        seen.add(row.key);
      }
    }
    for (const key of cache.keys()) {
      if (!seen.has(key)) {
        cache.delete(key);
      }
    }
    const newestFirst: Row[] = [];
    for (let i = rows.length - 1; i >= 0; i--) {
      const row = rows[i];
      if (row !== undefined) {
        newestFirst.push(row);
      }
    }
    return newestFirst;
  };
  return memoSelector(build, sameRows);
}

function sameRows(a: readonly Row[], b: readonly Row[]): boolean {
  return a.length === b.length && a.every((row, i) => row === b[i]);
}

function turnRows(
  turnId: string,
  turn: TurnDetails | undefined,
  stream: TurnView | undefined,
): Row[] {
  const rows: Row[] = [];
  if (turn?.prompt !== undefined) {
    rows.push({ kind: "prompt", key: `p:${turnId}`, turnId });
  }
  if (stream !== undefined && stream.reasoning.text.length > 0) {
    rows.push({ kind: "reasoning", key: `r:${turnId}`, turnId });
  }
  const messages = new Set<number>();
  const tools = new Set<string>();
  for (const part of turn?.parts ?? []) {
    switch (part.kind) {
      case "message":
        if (hasText(stream, part.index)) {
          messages.add(part.index);
          rows.push(messageRow(turnId, part.index));
        }
        break;
      case "tool":
        tools.add(part.callId);
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
        rows.push({ kind: "steer", key: `s:${turnId}:${part.seq}`, turnId, seq: part.seq });
        break;
    }
  }
  // Parts that the stream has and the details do not, as after a details store that
  // missed an item: they come last, in the stream's order.
  if (stream !== undefined) {
    for (const index of [...stream.messages.keys()].sort((a, b) => a - b)) {
      if (!messages.has(index) && hasText(stream, index)) {
        rows.push(messageRow(turnId, index));
      }
    }
    for (const callId of stream.toolCalls.keys()) {
      if (!tools.has(callId)) {
        rows.push({ kind: "tool", key: `t:${turnId}:${callId}`, turnId, callId });
      }
    }
  }
  const ended = turn?.end !== undefined || stream?.ended === true;
  if (!ended && (turn?.started === true || stream !== undefined)) {
    rows.push({ kind: "status", key: `w:${turnId}`, turnId });
  }
  if (turn?.end !== undefined) {
    rows.push({ kind: "end", key: `e:${turnId}`, turnId });
  }
  return rows;
}

function messageRow(turnId: string, index: number): Row {
  return { kind: "message", key: `m:${turnId}:${index}`, turnId, index };
}

function hasText(stream: TurnView | undefined, index: number): boolean {
  const message = stream?.messages.get(index);
  return message !== undefined && message.buffer.text.length > 0;
}
