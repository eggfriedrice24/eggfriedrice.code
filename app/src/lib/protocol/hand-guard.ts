// The hand-written envelope guard of protocol 1, as the app had it before the zod
// schemas (envelope.ts at commit 7a970f31). The app does not use it: it is only the
// baseline of the decode bench (scripts/bench-decode.mjs, and the Hermes bench of the dev
// build), so that the cost of the schemas has a number to compare with.
//
// It checks the envelope and the members that the app read, and trusts efrd for the rest
// (R6 item 10 of the research). An event, draft or item kind that it does not know
// decodes to an `other` wrapper. A known kind with a malformed member is an error.

import {
  DRAFT_PART_KINDS,
  ERROR_CODES,
  EVENT_KINDS,
  type ConversationSummary,
  type ErrorCode,
  type Event as KnownEvent,
  type DraftPart as KnownDraftPart,
} from "./generated";
import type {
  Draft,
  DraftPart,
  ErrorBody,
  Event,
  EventEnvelope,
  EventKind,
  LiveTurn,
  ServerFrame,
  Snapshot,
  SubscribeItem,
} from "./wire";
import type { Decoded } from "./envelope";

type Json = Readonly<Record<string, unknown>>;

class Invalid extends Error {}

const SERVER_SHAPES = ["item", "end", "error", "ack"] as const;

const EVENT_KIND_SET: ReadonlySet<string> = new Set<string>(EVENT_KINDS);
const DRAFT_PART_KIND_SET: ReadonlySet<string> = new Set<string>(DRAFT_PART_KINDS);
const ERROR_CODE_SET: ReadonlySet<string> = new Set<string>(ERROR_CODES);

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

function isEventKind(kind: string): kind is EventKind {
  return EVENT_KIND_SET.has(kind);
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

// The three functions below are the only places where the guard trusts efrd: each one
// runs after the guard checked the members that the app reads (R6 item 10).

function trustedEvent(object: unknown): KnownEvent {
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- checked by checkEvent.
  return object as KnownEvent;
}

function trustedDraftPart(object: unknown): KnownDraftPart {
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- a known kind; no member is read.
  return object as KnownDraftPart;
}

function trustedConversation(object: unknown): ConversationSummary {
  // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- checked by conversation.
  return object as ConversationSummary;
}

function serverFrame(value: unknown): ServerFrame {
  const object = record(value, "frame");
  const shapes = SERVER_SHAPES.filter((key) => key in object);
  if (shapes.length !== 1) {
    throw new Invalid("a server frame has exactly one of `item`, `end`, `error` and `ack`");
  }
  switch (shapes[0]) {
    case "item":
      return { type: "item", id: u64(object["id"], "id"), item: object["item"] ?? null };
    case "end":
      if (object["end"] !== true) {
        throw new Invalid("the `end` member must be `true`");
      }
      return { type: "end", id: u64(object["id"], "id") };
    case "error": {
      const id =
        object["id"] === undefined || object["id"] === null ? null : u64(object["id"], "id");
      return { type: "error", id, error: errorBody(object["error"]) };
    }
    case "ack":
      return { type: "ack", id: u64(object["ack"], "ack") };
    default:
      throw new Invalid("a server frame has exactly one of `item`, `end`, `error` and `ack`");
  }
}

function errorBody(value: unknown): ErrorBody {
  const object = record(value, "error");
  const body: ErrorBody = {
    code: errorCode(object["code"]),
    message: string(object["message"], "error.message"),
  };
  // The data of every code is an object. Other data says nothing that the app reads.
  const data = object["data"];
  if (isJsonObject(data)) {
    body.data = { ...data };
  }
  return body;
}

function errorCode(value: unknown): ErrorCode {
  const code = string(value, "error.code");
  if (!isErrorCode(code)) {
    throw new Invalid(`error.code ${code} is not a code of this protocol`);
  }
  return code;
}

function isErrorCode(code: string): code is ErrorCode {
  return ERROR_CODE_SET.has(code);
}

function subscribeItem(value: unknown): SubscribeItem {
  const object = record(value, "item");
  const kind = string(object["kind"], "item.kind");
  switch (kind) {
    case "event":
      return { kind: "event", ...eventEnvelope(object) };
    case "snapshot":
      return { kind: "snapshot", ...snapshot(object) };
    case "draft":
      return { kind: "draft", ...draft(object) };
    default:
      return { kind: "other", wireKind: kind };
  }
}

function eventEnvelope(value: unknown): EventEnvelope {
  const object = record(value, "event envelope");
  const envelope: { seq: number; at: string; event: Event; conversation_id?: string | null } = {
    seq: u64(object["seq"], "seq"),
    at: string(object["at"], "at"),
    event: event(object["event"]),
  };
  if (object["conversation_id"] !== undefined) {
    envelope.conversation_id = nullable(object["conversation_id"], string, "conversation_id");
  }
  return envelope;
}

function event(value: unknown): Event {
  const object = record(value, "event");
  const kind = string(object["kind"], "event.kind");
  if (!isEventKind(kind)) {
    return { kind: "other", wireKind: kind };
  }
  checkEvent(kind, object);
  return trustedEvent(object);
}

/** Checks the members that the app reads of an event of kind `kind`. */
function checkEvent(kind: EventKind, object: Json): void {
  const turnId = (): string => string(object["turn_id"], `${kind}.turn_id`);
  const callId = (): string => string(object["call_id"], `${kind}.call_id`);
  switch (kind) {
    case "turn_started":
    case "turn_completed":
    case "turn_failed":
    case "turn_interrupted":
    case "turn_cancelled":
      turnId();
      return;
    case "prompt_queued":
      turnId();
      string(object["text"], `${kind}.text`);
      string(object["origin"], `${kind}.origin`);
      return;
    case "assistant_message_updated":
      turnId();
      u32(object["index"], `${kind}.index`);
      u64(object["offset"], `${kind}.offset`);
      string(object["delta"], `${kind}.delta`);
      return;
    case "assistant_message_completed":
      turnId();
      u32(object["index"], `${kind}.index`);
      string(object["text"], `${kind}.text`);
      return;
    case "tool_call_started":
      turnId();
      callId();
      string(object["tool"], `${kind}.tool`);
      return;
    case "tool_call_output_updated":
      turnId();
      callId();
      string(object["tail"], `${kind}.tail`);
      u64(object["bytes"], `${kind}.bytes`);
      return;
    case "tool_call_completed":
      turnId();
      callId();
      return;
    case "approval_requested":
      turnId();
      callId();
      string(object["summary"], `${kind}.summary`);
      optional(object["local_only"], boolean, `${kind}.local_only`);
      optional(object["interactive"], boolean, `${kind}.interactive`);
      optional(object["expires_in_secs"], (member, what) => nullable(member, u64, what), kind);
      return;
    case "approval_resolved":
      turnId();
      callId();
      string(object["decision"], `${kind}.decision`);
      string(object["origin"], `${kind}.origin`);
      return;
    case "approval_expired":
      turnId();
      callId();
      optional(object["away"], boolean, `${kind}.away`);
      return;
    default:
      return;
  }
}

function draft(object: Json): Draft {
  return {
    turn_id: string(object["turn_id"], "draft.turn_id"),
    after_seq: u64(object["after_seq"], "draft.after_seq"),
    draft: draftPart(object["draft"]),
  };
}

function draftPart(value: unknown): DraftPart {
  const object = record(value, "draft.draft");
  const kind = string(object["kind"], "draft.draft.kind");
  switch (kind) {
    case "text":
      return {
        kind,
        index: u32(object["index"], "text.index"),
        offset: u64(object["offset"], "text.offset"),
        delta: string(object["delta"], "text.delta"),
      };
    case "reasoning": {
      const part: { kind: "reasoning"; offset: number; delta: string; title?: string | null } = {
        kind,
        offset: u64(object["offset"], "reasoning.offset"),
        delta: string(object["delta"], "reasoning.delta"),
      };
      if (object["title"] !== undefined) {
        part.title = nullable(object["title"], string, "reasoning.title");
      }
      return part;
    }
    case "tool_input":
      return {
        kind,
        call: u32(object["call"], "tool_input.call"),
        tool: string(object["tool"], "tool_input.tool"),
        bytes: u64(object["bytes"], "tool_input.bytes"),
      };
    default:
      return DRAFT_PART_KIND_SET.has(kind)
        ? trustedDraftPart(object)
        : { kind: "other", wireKind: kind };
  }
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
    conversation: conversation(object["conversation"]),
    events: events.map((envelope: unknown) => eventEnvelope(envelope)),
    hwm: u64(object["hwm"], "snapshot.hwm"),
  };
  if (object["history_cursor"] !== undefined) {
    result.history_cursor = nullable(object["history_cursor"], string, "snapshot.history_cursor");
  }
  if (object["live"] !== undefined) {
    result.live = nullable(object["live"], live, "snapshot.live");
  }
  return result;
}

function conversation(value: unknown): ConversationSummary {
  const object = record(value, "conversation");
  string(object["id"], "conversation.id");
  string(object["status"], "conversation.status");
  string(object["updated_at"], "conversation.updated_at");
  u64(object["last_seq"], "conversation.last_seq");
  optional(object["title"], (member, what) => nullable(member, string, what), "conversation.title");
  return trustedConversation(object);
}

function live(value: unknown): LiveTurn {
  const object = record(value, "snapshot.live");
  const result: LiveTurn = {
    turn_id: string(object["turn_id"], "live.turn_id"),
    after_seq: u64(object["after_seq"], "live.after_seq"),
    index: u32(object["index"], "live.index"),
    text: string(object["text"], "live.text"),
    text_bytes: u64(object["text_bytes"], "live.text_bytes"),
  };
  if (object["reasoning"] !== undefined) {
    result.reasoning = nullable(object["reasoning"], string, "live.reasoning");
  }
  if (object["reasoning_title"] !== undefined) {
    result.reasoning_title = nullable(object["reasoning_title"], string, "live.reasoning_title");
  }
  return result;
}

function record(value: unknown, what: string): Json {
  if (!isJsonObject(value)) {
    throw new Invalid(`${what} is not an object`);
  }
  return value;
}

function isJsonObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function string(value: unknown, what: string): string {
  if (typeof value !== "string") {
    throw new Invalid(`${what} is not a string`);
  }
  return value;
}

function boolean(value: unknown, what: string): boolean {
  if (typeof value !== "boolean") {
    throw new Invalid(`${what} is not a boolean`);
  }
  return value;
}

function nullable<T>(
  value: unknown,
  decode: (value: unknown, what: string) => T,
  what: string,
): T | null {
  return value === null ? null : decode(value, what);
}

function optional<T>(
  value: unknown,
  decode: (value: unknown, what: string) => T,
  what: string,
): T | undefined {
  return value === undefined ? undefined : decode(value, what);
}

function u64(value: unknown, what: string): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new Invalid(`${what} is not an integer from 0 to 2^53 - 1`);
  }
  return value;
}

function u32(value: unknown, what: string): number {
  const number = u64(value, what);
  if (number > 0xffff_ffff) {
    throw new Invalid(`${what} is larger than a u32`);
  }
  return number;
}
