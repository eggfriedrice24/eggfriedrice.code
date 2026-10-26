// A text that grows by deltas at UTF-8 byte offsets, as an assistant message or the
// reasoning of a turn does. Drafts and persisted updates count the same bytes, so one
// buffer merges both:
//
// - A delta at `offset == bytes` appends.
// - A delta at `offset < bytes` overlaps: the buffer trims the overlap by bytes and
//   appends the rest.
// - A delta at `offset > bytes` leaves a gap: the buffer keeps it pending, and applies it
//   when a later delta, a seed or the completed text fills the gap.
//
// Pending deltas that touch each other merge into one run, so a long stream that waits
// for a gap keeps a few runs, not one entry per draft. The functions are pure and
// return the same object when nothing changed.

import { dropUtf8Prefix, utf8Length } from "./utf8";

/** A run of text that starts past the end of the buffer. */
export interface PendingRun {
  readonly offset: number;
  readonly text: string;
  readonly bytes: number;
}

/** A text that grows by byte-offset deltas. */
export interface TextBuffer {
  readonly text: string;
  /** The UTF-8 length of `text`. */
  readonly bytes: number;
  /** Runs past the end of `text`, by offset, never touching each other or `text`. */
  readonly pending: readonly PendingRun[];
}

/** The most pending runs a buffer keeps. Past it, the run with the largest offset goes. */
export const MAX_PENDING_RUNS = 16;

/** An empty buffer. */
export const EMPTY_BUFFER: TextBuffer = { text: "", bytes: 0, pending: [] };

/** A buffer that holds `text`. */
export function bufferOf(text: string): TextBuffer {
  return { text, bytes: utf8Length(text), pending: [] };
}

/** Applies `delta` at byte `offset`. */
export function applyDelta(buffer: TextBuffer, offset: number, delta: string): TextBuffer {
  if (delta.length === 0) {
    return buffer;
  }
  if (offset > buffer.bytes) {
    return addPending(buffer, { offset, text: delta, bytes: utf8Length(delta) });
  }
  return drain(appendAt(buffer, offset, delta, utf8Length(delta)));
}

/**
 * Replaces the text with `text` when it holds more than the buffer, and keeps the
 * pending runs that are still past its end. A seed comes from the snapshot or from a
 * page of history, which hold the text so far.
 */
export function seed(buffer: TextBuffer, text: string): TextBuffer {
  const bytes = utf8Length(text);
  if (bytes <= buffer.bytes) {
    return buffer;
  }
  return drain({ text, bytes, pending: buffer.pending });
}

function appendAt(buffer: TextBuffer, offset: number, delta: string, bytes: number): TextBuffer {
  const overlap = buffer.bytes - offset;
  if (overlap >= bytes) {
    return buffer;
  }
  const rest = overlap === 0 ? delta : dropUtf8Prefix(delta, overlap);
  if (rest === undefined) {
    // NOTE: the overlap ends inside a character, so the delta does not fit the text. efrd
    // never sends such a delta; the completed text repairs the buffer.
    return buffer;
  }
  return {
    text: buffer.text + rest,
    bytes: buffer.bytes + bytes - overlap,
    pending: buffer.pending,
  };
}

function drain(buffer: TextBuffer): TextBuffer {
  let current = buffer;
  while (current.pending.length > 0) {
    const first = current.pending[0];
    if (first === undefined || first.offset > current.bytes) {
      break;
    }
    const rest = current.pending.slice(1);
    current = appendAt({ ...current, pending: rest }, first.offset, first.text, first.bytes);
  }
  return current;
}

function addPending(buffer: TextBuffer, run: PendingRun): TextBuffer {
  // A fresh array, so the sort changes no array that another state still holds.
  const runs = [...buffer.pending, run].sort((a, b) => a.offset - b.offset);
  const merged: PendingRun[] = [];
  for (const next of runs) {
    const last = merged[merged.length - 1];
    if (last !== undefined && next.offset <= last.offset + last.bytes) {
      merged[merged.length - 1] = mergeRuns(last, next);
    } else {
      merged.push(next);
    }
  }
  while (merged.length > MAX_PENDING_RUNS) {
    merged.pop();
  }
  return { ...buffer, pending: merged };
}

function mergeRuns(first: PendingRun, second: PendingRun): PendingRun {
  const grown = appendAt(
    { text: first.text, bytes: first.bytes, pending: [] },
    second.offset - first.offset,
    second.text,
    second.bytes,
  );
  return { offset: first.offset, text: grown.text, bytes: grown.bytes };
}
