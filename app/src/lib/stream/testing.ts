// Helpers for the tests of the stream: typed items with every member that efrd always
// sends, so a test feeds the reducer what the decoder gives it. Only test files import
// this module.

import type { ConversationSummary, ConversationStatus } from "@/lib/protocol/generated";
import type {
  DraftPart,
  Event,
  EventEnvelope,
  EventOf,
  LiveTurn,
  Snapshot,
  SubscribeItem,
} from "@/lib/protocol/wire";
import { EMPTY_STREAM, reduce, type StreamState } from "./reducer";
import { utf8Length } from "./utf8";

/** The time of the test items, as efrd writes it. */
export const AT = "2026-10-26T12:00:00Z";

/** A manual frame clock: `frame()` runs the callbacks of the next frame. */
export interface ManualFrames {
  readonly schedule: (callback: () => void) => () => void;
  /** Runs the callbacks that wait for the next frame. */
  frame(): void;
  /** How many callbacks wait for the next frame. */
  pending(): number;
}

/** A new manual frame clock. */
export function manualFrames(): ManualFrames {
  let queue: (() => void)[] = [];
  return {
    schedule: (callback) => {
      queue.push(callback);
      return () => {
        queue = queue.filter((other) => other !== callback);
      };
    },
    frame: () => {
      const now = queue;
      queue = [];
      for (const callback of now) {
        callback();
      }
    },
    pending: () => queue.length,
  };
}

/** `AT` in ms since the epoch. */
export const AT_MS = Date.parse(AT);

/** `AT` plus `seconds`, as efrd writes it. */
export function at(seconds: number): string {
  return new Date(AT_MS + seconds * 1000).toISOString();
}

/** One delta of a text: `delta` at UTF-8 byte `offset`. */
export interface Chunk {
  readonly offset: number;
  readonly delta: string;
}

/**
 * Splits `text` into deltas at code point boundaries. Each value of `cuts` picks one cut
 * point; equal cuts give one cut. The deltas are in order and together give `text`.
 */
export function chunked(text: string, cuts: readonly number[]): Chunk[] {
  const points = Array.from(text);
  const picked = [...new Set(cuts.map((cut) => cut % (points.length + 1)))].sort((a, b) => a - b);
  const bounds = [0, ...picked.filter((cut) => cut > 0 && cut < points.length), points.length];
  const chunks: Chunk[] = [];
  let offset = 0;
  for (let i = 1; i < bounds.length; i++) {
    const delta = points.slice(bounds[i - 1], bounds[i]).join("");
    if (delta.length > 0) {
      chunks.push({ offset, delta });
      offset += utf8Length(delta);
    }
  }
  return chunks;
}

/** An event item at `seq`. */
export function eventItem(seq: number, body: Event, time = AT): SubscribeItem {
  return { kind: "event", seq, at: time, event: body };
}

/** An event envelope at `seq`, as a snapshot or the cache holds it. */
export function envelope(seq: number, body: Event, time = AT): EventEnvelope {
  return { seq, at: time, event: body };
}

/** A draft item of `turnId`. */
export function draftItem(turnId: string, afterSeq: number, part: DraftPart): SubscribeItem {
  return { kind: "draft", turn_id: turnId, after_seq: afterSeq, draft: part };
}

/** A text draft part. */
export function textPart(index: number, offset: number, delta: string): DraftPart {
  return { kind: "text", index, offset, delta };
}

/** A summary of the conversation, as a snapshot carries it. */
export function summary(
  id: string,
  status: ConversationStatus,
  lastSeq: number,
): ConversationSummary {
  return { id, status, created_at: AT, updated_at: AT, last_seq: lastSeq };
}

/** A snapshot item. */
export function snapshotItem(
  conversationId: string,
  status: ConversationStatus,
  events: readonly EventEnvelope[],
  hwm: number,
  live?: LiveTurn,
): SubscribeItem {
  const item: Snapshot & { readonly kind: "snapshot" } = {
    kind: "snapshot",
    conversation: summary(conversationId, status, hwm),
    events: [...events],
    hwm,
  };
  if (live !== undefined) {
    item.live = live;
  }
  return item;
}

/** Applies `items` in order. */
export function run(
  items: readonly SubscribeItem[],
  from: StreamState = EMPTY_STREAM,
): StreamState {
  return items.reduce(reduce, from);
}

/** A `prompt_queued` event from `origin`. */
export function promptQueued(
  turnId: string,
  text: string,
  origin: EventOf<"prompt_queued">["origin"] = "shell",
): EventOf<"prompt_queued"> {
  return { kind: "prompt_queued", turn_id: turnId, command_id: `cmd-${turnId}`, origin, text };
}

/** A `turn_started` event with the members that efrd always sends. */
export function turnStarted(turnId: string): EventOf<"turn_started"> {
  return { kind: "turn_started", turn_id: turnId, cwd: "/home/me", scope: { kind: "machine" } };
}

/** A `tool_call_started` event with an empty input. */
export function toolCallStarted(
  turnId: string,
  callId: string,
  tool: string,
  input: Record<string, unknown> = {},
): EventOf<"tool_call_started"> {
  return { kind: "tool_call_started", turn_id: turnId, call_id: callId, tool, input };
}

/** A `tool_call_completed` event that succeeded with `output`. */
export function toolCallCompleted(
  turnId: string,
  callId: string,
  output = "",
): EventOf<"tool_call_completed"> {
  return {
    kind: "tool_call_completed",
    turn_id: turnId,
    call_id: callId,
    output,
    truncated: false,
    is_error: false,
  };
}

/** An `approval_requested` event. */
export function approvalRequested(
  turnId: string,
  callId: string,
  summaryText = `run ${callId}`,
): EventOf<"approval_requested"> {
  return { kind: "approval_requested", turn_id: turnId, call_id: callId, summary: summaryText };
}

/** An `approval_resolved` event. */
export function approvalResolved(
  turnId: string,
  callId: string,
  decision: EventOf<"approval_resolved">["decision"],
  origin: EventOf<"approval_resolved">["origin"] = "phone",
): EventOf<"approval_resolved"> {
  return { kind: "approval_resolved", turn_id: turnId, call_id: callId, decision, origin };
}

/** An `assistant_message_completed` event. */
export function messageCompleted(
  turnId: string,
  index: number,
  text: string,
): EventOf<"assistant_message_completed"> {
  return { kind: "assistant_message_completed", turn_id: turnId, index, text };
}

/** An `assistant_message_updated` event. */
export function messageUpdated(
  turnId: string,
  index: number,
  offset: number,
  delta: string,
): EventOf<"assistant_message_updated"> {
  return { kind: "assistant_message_updated", turn_id: turnId, index, offset, delta };
}

/** A `turn_completed` event. */
export function turnCompleted(turnId: string): EventOf<"turn_completed"> {
  return { kind: "turn_completed", turn_id: turnId };
}
