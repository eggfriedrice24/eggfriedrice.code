// Small local copies of the wire types that the core reads. They follow the JSON of
// efr-protocol (crates/efr-protocol). The generated types of docs/protocol.schema.json
// replace them later; until then, keep each field name and meaning the same as there.
//
// Numbers: efrd sends u64 values (seq, offset, request id). The app keeps them as JS
// numbers, which are exact up to 2^53. The envelope guard refuses a larger value.

/** A server frame, told apart by its members, as efr-protocol's `ServerFrame`. */
export type ServerFrame =
  | { readonly type: "item"; readonly id: number; readonly item: unknown }
  | { readonly type: "end"; readonly id: number }
  | { readonly type: "error"; readonly id: number | null; readonly error: ErrorBody }
  | { readonly type: "ack"; readonly id: number };

/** The body of an error frame. */
export interface ErrorBody {
  readonly code: string;
  readonly message: string;
  readonly data?: unknown;
}

/** One event with its place in the log (`EventEnvelope`). */
export interface EventEnvelope {
  readonly seq: number;
  readonly conversation_id?: string;
  readonly at: string;
  readonly event: Event;
}

/** The events that the stream reducer reads. Every other kind is `OtherEvent`. */
export type Event =
  | {
      readonly kind: "turn_started";
      readonly turn_id: string;
    }
  | {
      readonly kind: "assistant_message_updated";
      readonly turn_id: string;
      readonly index: number;
      readonly offset: number;
      readonly delta: string;
    }
  | {
      readonly kind: "assistant_message_completed";
      readonly turn_id: string;
      readonly index: number;
      readonly text: string;
    }
  | {
      readonly kind: "tool_call_started";
      readonly turn_id: string;
      readonly call_id: string;
      readonly tool: string;
    }
  | {
      readonly kind: "tool_call_output_updated";
      readonly turn_id: string;
      readonly call_id: string;
      readonly tail: string;
      readonly bytes: number;
    }
  | {
      readonly kind: "tool_call_completed";
      readonly turn_id: string;
      readonly call_id: string;
    }
  | {
      readonly kind: "turn_completed" | "turn_failed" | "turn_interrupted" | "turn_cancelled";
      readonly turn_id: string;
    }
  | OtherEvent;

/** An event kind that the stream reducer does not read, or one this build does not know. */
export interface OtherEvent {
  readonly kind: "other";
  /** The kind as it came on the wire. */
  readonly wireKind: string;
}

/** The part of a running turn that a draft carries (`DraftPart`). */
export type DraftPart =
  | {
      readonly kind: "text";
      readonly index: number;
      readonly offset: number;
      readonly delta: string;
    }
  | {
      readonly kind: "reasoning";
      readonly offset: number;
      readonly delta: string;
      readonly title?: string;
    }
  | {
      readonly kind: "tool_input";
      readonly call: number;
      readonly tool: string;
      readonly bytes: number;
    }
  | { readonly kind: "other"; readonly wireKind: string };

/** A draft: part of a running turn before the event log has it (`Draft`). */
export interface Draft {
  readonly turn_id: string;
  readonly after_seq: number;
  readonly draft: DraftPart;
}

/**
 * The partial state of the running turn in a snapshot. W1-PROTO defines the wire shape
 * (`ConversationSnapshot.live`); this local shape holds what the reducer needs.
 */
export interface LiveMessage {
  readonly turn_id: string;
  readonly index: number;
  readonly text: string;
  readonly reasoning?: string;
  readonly title?: string;
}

/** A bounded view of a conversation (`ConversationSnapshot`), reduced to what the core reads. */
export interface Snapshot {
  readonly events: readonly EventEnvelope[];
  readonly hwm: number;
  readonly live?: LiveMessage;
}

/**
 * One item of a `conversation.subscribe` stream (`ConversationSubscribeItem`). On the
 * wire the `kind` member sits next to the members of the item, as here.
 */
export type SubscribeItem =
  | (EventEnvelope & { readonly kind: "event" })
  | (Snapshot & { readonly kind: "snapshot" })
  | (Draft & { readonly kind: "draft" })
  | { readonly kind: "other"; readonly wireKind: string };
