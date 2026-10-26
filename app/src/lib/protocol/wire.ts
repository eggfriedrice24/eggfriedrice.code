// The wire types that the core reads. They are the generated types of efr-protocol
// (src/lib/protocol/generated.ts, from docs/protocol.schema.json), plus a skip wrapper for
// each kind that this build does not know.
//
// Forward compatibility (R3.12 of the research): a newer efrd can send an event, draft
// part or item kind that the generated file does not list. The decoder (envelope.ts)
// decodes it to `OtherKind`, which the reducer and the store skip. A known event or
// draft part whose body fails its schema is an `OtherKind` with `invalid: true`.
//
// Numbers: efrd sends u64 values (seq, offset, request id). The app keeps them as JS
// numbers, which are exact up to 2^53. The decoder refuses a larger value.

import type * as Protocol from "./generated";

/**
 * A kind that this build does not know, or a known kind that failed its schema, kept
 * only so that the app can skip it.
 */
export interface OtherKind {
  readonly kind: "other";
  /** The kind as it came on the wire. */
  readonly wireKind: string;
  /**
   * True for a known kind whose members failed its schema, for example a new value of
   * an enum from a newer efrd. Absent for a kind that this build does not know.
   */
  readonly invalid?: true;
}

/** An event kind that this build does not know. */
export type OtherEvent = OtherKind;

/** The body of an error frame. */
export type ErrorBody = Protocol.ErrorBody;

/** One event of the log, or the skip wrapper. */
export type Event = Protocol.Event | OtherEvent;

/** The event kinds of this build. */
export type EventKind = Protocol.Event["kind"];

/** The variant of `Event` with kind `K`. */
export type EventOf<K extends EventKind> = Extract<Protocol.Event, { kind: K }>;

/** One event with its place in the log (`EventEnvelope`). */
export type EventEnvelope = Omit<Protocol.EventEnvelope, "event"> & { readonly event: Event };

/** The part of a running turn that a draft carries (`DraftPart`), or the skip wrapper. */
export type DraftPart = Protocol.DraftPart | OtherKind;

/** A draft: part of a running turn before the event log has it (`Draft`). */
export type Draft = Omit<Protocol.Draft, "draft"> & { readonly draft: DraftPart };

/** The state of the running turn in a snapshot (`LiveTurn`). */
export type LiveTurn = Protocol.LiveTurn;

/** A bounded view of a conversation (`ConversationSnapshot`). */
export type Snapshot = Omit<Protocol.ConversationSnapshot, "events"> & {
  readonly events: readonly EventEnvelope[];
};

/** The members of a snapshot that the stream reducer reads. */
export type SnapshotView = Pick<Snapshot, "events" | "hwm"> & {
  readonly live?: LiveTurn | null | undefined;
};

/**
 * One item of a `conversation.subscribe` stream (`ConversationSubscribeItem`). On the
 * wire the `kind` member sits next to the members of the item, as here.
 */
export type SubscribeItem =
  | (EventEnvelope & { readonly kind: "event" })
  | (Snapshot & { readonly kind: "snapshot" })
  | (Draft & { readonly kind: "draft" })
  | OtherKind;

/** The history page of `conversation.history`, whose events may hold skip wrappers. */
export type HistoryPage = Omit<Protocol.ConversationHistoryResult, "events"> & {
  readonly events: EventEnvelope[];
  /**
   * The events as efrd sent them, in the same order, for the event cache: a later build
   * of the app can read an event that this one skips. Absent for a page that the app
   * made itself, such as one of the demo machines.
   */
  readonly raw?: readonly unknown[];
};

/**
 * The result of each method as the app reads it: the generated `MethodResults`, with the
 * history page whose events may hold skip wrappers.
 */
export type CallResults = {
  readonly [M in keyof Protocol.MethodResults]: M extends "conversation.history"
    ? HistoryPage
    : Protocol.MethodResults[M];
};

/**
 * A server frame as the envelope guard decodes it. On the wire the shapes are told
 * apart by their members (`ServerFrame`); `type` names the member that the frame has.
 */
export type ServerFrame =
  | { readonly type: "item"; readonly id: Protocol.RequestId; readonly item: unknown }
  | { readonly type: "end"; readonly id: Protocol.RequestId }
  | { readonly type: "error"; readonly id: Protocol.RequestId | null; readonly error: ErrorBody }
  | { readonly type: "ack"; readonly id: Protocol.RequestId };
