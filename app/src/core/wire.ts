// The wire types that the core reads. They are the generated types of efr-protocol
// (src/protocol/generated.ts, from docs/protocol.schema.json), plus a skip wrapper for
// each kind that this build does not know.
//
// Forward compatibility (R3.12 of the research): a newer efrd can send an event, draft
// part or item kind that the generated file does not list. The envelope guard decodes
// it to `OtherKind`, which the reducer and the store skip. A known kind with a
// malformed body is an error.
//
// Numbers: efrd sends u64 values (seq, offset, request id). The app keeps them as JS
// numbers, which are exact up to 2^53. The envelope guard refuses a larger value.

import type * as Protocol from "../protocol/generated";

/** A kind that this build does not know, kept only so that the app can skip it. */
export interface OtherKind {
  readonly kind: "other";
  /** The kind as it came on the wire. */
  readonly wireKind: string;
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

/**
 * A server frame as the envelope guard decodes it. On the wire the shapes are told
 * apart by their members (`ServerFrame`); `type` names the member that the frame has.
 */
export type ServerFrame =
  | { readonly type: "item"; readonly id: Protocol.RequestId; readonly item: unknown }
  | { readonly type: "end"; readonly id: Protocol.RequestId }
  | { readonly type: "error"; readonly id: Protocol.RequestId | null; readonly error: ErrorBody }
  | { readonly type: "ack"; readonly id: Protocol.RequestId };
