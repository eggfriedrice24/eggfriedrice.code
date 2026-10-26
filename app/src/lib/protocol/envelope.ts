// The decoder: it checks each frame from efrd with the generated zod schemas
// (generated-zod.ts) before the app reads it.
//
// Forward compatibility, as efr-protocol does it: an event, draft part or item kind that
// this build does not know decodes to an `other` wrapper, which the app skips. A known
// kind whose members fail its schema, for example a new value of an enum that
// efr-protocol can grow, decodes to an `other` wrapper with `invalid: true` and logs once
// in a dev build. The stream goes on after it: the envelope of such an event still
// carries its `seq`, so the cursor moves past it and no new subscribe asks for it again.
//
// What must be valid, or the decode fails: the frame, the members of an event envelope
// (`seq`, `at`, `conversation_id`), and a snapshot with its conversation summary. A
// draft is best effort, so a draft with bad members is an invalid `other` item instead.
//
// A page of `conversation.history` follows the rule of the stream: an event of a kind
// that the app does not know, or a known one that fails its schema, is an `other` event
// in the page, and the rest of the page stays.

import type { z } from "zod";

import { DRAFT_PART_KINDS, EVENT_KINDS, type ConversationSummary } from "./generated";
import {
  conversationHistoryResultSchema,
  conversationSnapshotSchema,
  conversationSummarySchema,
  draftPartSchema,
  draftSchema,
  errorBodySchema,
  errorFrameSchema,
  eventEnvelopeSchema,
  eventSchema,
  serverAckFrameSchema,
  serverEndFrameSchema,
  serverItemFrameSchema,
} from "./generated-zod";
import type {
  Draft,
  DraftPart,
  ErrorBody,
  Event,
  EventEnvelope,
  EventKind,
  HistoryPage,
  LiveTurn,
  OtherKind,
  ServerFrame,
  Snapshot,
  SubscribeItem,
} from "./wire";

/** The result of a decode: the value, or why the input is not valid. */
export type Decoded<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly reason: string };

type Json = Readonly<Record<string, unknown>>;

class Invalid extends Error {}

const SERVER_SHAPES = ["item", "end", "error", "ack"] as const;

const EVENT_KIND_SET: ReadonlySet<string> = new Set<string>(EVENT_KINDS);
const DRAFT_PART_KIND_SET: ReadonlySet<string> = new Set<string>(DRAFT_PART_KINDS);

// The member schemas, so that each member gets its own reason.
const ENVELOPE = eventEnvelopeSchema.shape;
const DRAFT = draftSchema.shape;
const SNAPSHOT = conversationSnapshotSchema.shape;
const HISTORY = conversationHistoryResultSchema.shape;

/** The known kinds that failed their schema and that a dev build logged. */
const loggedInvalid = new Set<string>();

/** Decodes one text message from efrd into a server frame. */
export function decodeServerFrame(text: string): Decoded<ServerFrame> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return { ok: false, reason: "the frame is not JSON" };
  }
  return guard(() => serverFrame(value));
}

/** Decodes the `item` of a `conversation.subscribe` item frame. */
export function decodeSubscribeItem(item: unknown): Decoded<SubscribeItem> {
  return guard(() => subscribeItem(item));
}

/** Decodes one event envelope, such as an item of `conversation.history`. */
export function decodeEventEnvelope(value: unknown): Decoded<EventEnvelope> {
  return guard(() => eventEnvelope(value));
}

/**
 * Decodes the result of `conversation.history`. An event that the app cannot read is an
 * `other` event of the page; only a bad envelope or a bad cursor fails the page.
 */
export function decodeHistoryPage(value: unknown): Decoded<HistoryPage> {
  return guard(() => historyPage(value));
}

/** Decodes the body of an error frame. */
export function decodeErrorBody(value: unknown): Decoded<ErrorBody> {
  return guard(() => check(errorBodySchema, value, "error"));
}

/** Decodes a conversation summary, as `conversations.list` and a snapshot carry it. */
export function decodeConversationSummary(value: unknown): Decoded<ConversationSummary> {
  return guard(() => check(conversationSummarySchema, value, "conversation"));
}

/** True for a kind that the generated types list. */
export function isEventKind(kind: string): kind is EventKind {
  return EVENT_KIND_SET.has(kind);
}

/** True for a JSON object: not null and not an array. */
export function isJsonObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Why a value failed a schema, in one line: the place of the first problem and its
 * message, such as `seq: Too small: expected number to be >=0`.
 */
export function failureReason(what: string, error: z.ZodError): string {
  const issue = error.issues[0];
  if (issue === undefined) {
    return `${what} is not valid`;
  }
  const place = [what, ...issue.path.map((key) => String(key))].join(".");
  return `${place}: ${issue.message}`;
}

function guard<T>(decode: () => T): Decoded<T> {
  try {
    return { ok: true, value: decode() };
  } catch (error) {
    if (error instanceof Invalid) {
      return { ok: false, reason: error.message };
    }
    throw error;
  }
}

/** The value of `schema`, or an `Invalid` error that names `what`. */
function check<T>(schema: z.ZodType<T>, value: unknown, what: string): T {
  const result = schema.safeParse(value);
  if (!result.success) {
    throw new Invalid(failureReason(what, result.error));
  }
  return result.data;
}

/** The skip wrapper of a known kind that failed its schema. A dev build logs it once. */
function invalidKind(wireKind: string, reason: string): OtherKind {
  if (!loggedInvalid.has(wireKind)) {
    loggedInvalid.add(wireKind);
    if (typeof __DEV__ === "boolean" && __DEV__) {
      console.warn(`efr: the app skips a ${wireKind} that it cannot read (${reason})`);
    }
  }
  return { kind: "other", wireKind, invalid: true };
}

function record(value: unknown, what: string): Json {
  if (!isJsonObject(value)) {
    throw new Invalid(`${what} is not an object`);
  }
  return value;
}

function kindOf(object: Json, what: string): string {
  const kind = object["kind"];
  if (typeof kind !== "string") {
    throw new Invalid(`${what}.kind is not a string`);
  }
  return kind;
}

function serverFrame(value: unknown): ServerFrame {
  const object = record(value, "frame");
  const shapes = SERVER_SHAPES.filter((key) => key in object);
  if (shapes.length !== 1) {
    throw new Invalid("a server frame has exactly one of `item`, `end`, `error` and `ack`");
  }
  switch (shapes[0]) {
    case "item": {
      const frame = check(serverItemFrameSchema, object, "item frame");
      return { type: "item", id: frame.id, item: frame.item };
    }
    case "end":
      return { type: "end", id: check(serverEndFrameSchema, object, "end frame").id };
    case "error": {
      const frame = check(errorFrameSchema, object, "error frame");
      return { type: "error", id: frame.id ?? null, error: frame.error };
    }
    case "ack":
      return { type: "ack", id: check(serverAckFrameSchema, object, "ack frame").ack };
    default:
      throw new Invalid("a server frame has exactly one of `item`, `end`, `error` and `ack`");
  }
}

function subscribeItem(value: unknown): SubscribeItem {
  const object = record(value, "item");
  const kind = kindOf(object, "item");
  switch (kind) {
    case "event":
      return { kind: "event", ...eventEnvelope(object) };
    case "snapshot":
      return { kind: "snapshot", ...snapshot(object) };
    case "draft":
      return draftItem(object);
    default:
      return { kind: "other", wireKind: kind };
  }
}

function historyPage(value: unknown): HistoryPage {
  const object = record(value, "the conversation.history result");
  const events = object["events"];
  if (!Array.isArray(events)) {
    throw new Invalid("the conversation.history result.events is not an array");
  }
  const page: { events: EventEnvelope[]; raw: readonly unknown[]; next_cursor?: string | null } = {
    events: events.map((envelope: unknown) => eventEnvelope(envelope)),
    raw: events,
  };
  if (object["next_cursor"] !== undefined) {
    page.next_cursor = check(
      HISTORY.next_cursor,
      object["next_cursor"],
      "the conversation.history result.next_cursor",
    );
  }
  return page;
}

function eventEnvelope(value: unknown): EventEnvelope {
  const object = record(value, "event envelope");
  const envelope: { seq: number; at: string; event: Event; conversation_id?: string | null } = {
    seq: check(ENVELOPE.seq, object["seq"], "seq"),
    at: check(ENVELOPE.at, object["at"], "at"),
    event: event(object["event"]),
  };
  if (object["conversation_id"] !== undefined) {
    envelope.conversation_id = check(
      ENVELOPE.conversation_id,
      object["conversation_id"],
      "conversation_id",
    );
  }
  return envelope;
}

function event(value: unknown): Event {
  const object = record(value, "event");
  const kind = kindOf(object, "event");
  if (!isEventKind(kind)) {
    return { kind: "other", wireKind: kind };
  }
  const result = eventSchema.safeParse(object);
  return result.success ? result.data : invalidKind(kind, failureReason(kind, result.error));
}

function draftItem(object: Json): SubscribeItem {
  const turnId = DRAFT.turn_id.safeParse(object["turn_id"]);
  const afterSeq = DRAFT.after_seq.safeParse(object["after_seq"]);
  const part = object["draft"];
  if (!turnId.success) {
    return invalidKind("draft", failureReason("draft.turn_id", turnId.error));
  }
  if (!afterSeq.success) {
    return invalidKind("draft", failureReason("draft.after_seq", afterSeq.error));
  }
  if (!isJsonObject(part) || typeof part["kind"] !== "string") {
    return invalidKind("draft", "draft.draft is not an object with a kind");
  }
  const draft: Draft = { turn_id: turnId.data, after_seq: afterSeq.data, draft: draftPart(part) };
  return { kind: "draft", ...draft };
}

function draftPart(object: Json): DraftPart {
  const kind = kindOf(object, "draft.draft");
  if (!DRAFT_PART_KIND_SET.has(kind)) {
    return { kind: "other", wireKind: kind };
  }
  const result = draftPartSchema.safeParse(object);
  return result.success ? result.data : invalidKind(kind, failureReason(kind, result.error));
}

function snapshot(object: Json): Snapshot {
  const events = object["events"];
  if (!Array.isArray(events)) {
    throw new Invalid("snapshot.events is not an array");
  }
  const result: {
    conversation: ConversationSummary;
    events: EventEnvelope[];
    hwm: number;
    history_cursor?: string | null;
    live?: LiveTurn | null;
  } = {
    conversation: check(SNAPSHOT.conversation, object["conversation"], "snapshot.conversation"),
    events: events.map((envelope: unknown) => eventEnvelope(envelope)),
    hwm: check(SNAPSHOT.hwm, object["hwm"], "snapshot.hwm"),
  };
  if (object["history_cursor"] !== undefined) {
    result.history_cursor = check(
      SNAPSHOT.history_cursor,
      object["history_cursor"],
      "snapshot.history_cursor",
    );
  }
  if (object["live"] !== undefined) {
    result.live = check(SNAPSHOT.live, object["live"], "snapshot.live");
  }
  return result;
}
