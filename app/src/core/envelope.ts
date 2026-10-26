// The envelope guard: it checks the shape of each frame from efrd before the app reads
// it. It checks the envelope and the members that the stream reducer reads, not every
// member of every type: the app trusts efrd for the rest (R6 item 10 of the research).
//
// Forward compatibility, as efr-protocol does it: an event, draft or item kind that this
// build does not know decodes to an `other` wrapper, which the reducer skips. A known
// kind with a malformed body is an error.

import type {
  Draft,
  DraftPart,
  ErrorBody,
  Event,
  EventEnvelope,
  LiveMessage,
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
      const id = object["id"] === undefined ? null : u64(object["id"], "id");
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
  const body: { code: string; message: string; data?: unknown } = {
    code: string(object["code"], "error.code"),
    message: string(object["message"], "error.message"),
  };
  if (object["data"] !== undefined) {
    body.data = object["data"];
  }
  return body;
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
  const envelope: { seq: number; at: string; event: Event; conversation_id?: string } = {
    seq: u64(object["seq"], "seq"),
    at: string(object["at"], "at"),
    event: event(object["event"]),
  };
  if (object["conversation_id"] !== undefined) {
    envelope.conversation_id = string(object["conversation_id"], "conversation_id");
  }
  return envelope;
}

function event(value: unknown): Event {
  const object = record(value, "event");
  const kind = string(object["kind"], "event.kind");
  switch (kind) {
    case "turn_started":
    case "turn_completed":
    case "turn_failed":
    case "turn_interrupted":
    case "turn_cancelled":
      return { kind, turn_id: string(object["turn_id"], `${kind}.turn_id`) };
    case "assistant_message_updated":
      return {
        kind,
        turn_id: string(object["turn_id"], `${kind}.turn_id`),
        index: u32(object["index"], `${kind}.index`),
        offset: u64(object["offset"], `${kind}.offset`),
        delta: string(object["delta"], `${kind}.delta`),
      };
    case "assistant_message_completed":
      return {
        kind,
        turn_id: string(object["turn_id"], `${kind}.turn_id`),
        index: u32(object["index"], `${kind}.index`),
        text: string(object["text"], `${kind}.text`),
      };
    case "tool_call_started":
      return {
        kind,
        turn_id: string(object["turn_id"], `${kind}.turn_id`),
        call_id: string(object["call_id"], `${kind}.call_id`),
        tool: string(object["tool"], `${kind}.tool`),
      };
    case "tool_call_output_updated":
      return {
        kind,
        turn_id: string(object["turn_id"], `${kind}.turn_id`),
        call_id: string(object["call_id"], `${kind}.call_id`),
        tail: string(object["tail"], `${kind}.tail`),
        bytes: u64(object["bytes"], `${kind}.bytes`),
      };
    case "tool_call_completed":
      return {
        kind,
        turn_id: string(object["turn_id"], `${kind}.turn_id`),
        call_id: string(object["call_id"], `${kind}.call_id`),
      };
    default:
      return { kind: "other", wireKind: kind };
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
      const part: { kind: "reasoning"; offset: number; delta: string; title?: string } = {
        kind,
        offset: u64(object["offset"], "reasoning.offset"),
        delta: string(object["delta"], "reasoning.delta"),
      };
      if (object["title"] !== undefined) {
        part.title = string(object["title"], "reasoning.title");
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
      return { kind: "other", wireKind: kind };
  }
}

function snapshot(object: Json): Snapshot {
  const events = object["events"];
  if (!Array.isArray(events)) {
    throw new Invalid("snapshot.events is not an array");
  }
  const result: { events: EventEnvelope[]; hwm: number; live?: LiveMessage } = {
    events: events.map((envelope: unknown) => eventEnvelope(envelope)),
    hwm: u64(object["hwm"], "snapshot.hwm"),
  };
  if (object["live"] !== undefined && object["live"] !== null) {
    result.live = live(object["live"]);
  }
  return result;
}

function live(value: unknown): LiveMessage {
  const object = record(value, "snapshot.live");
  const result: {
    turn_id: string;
    index: number;
    text: string;
    reasoning?: string;
    title?: string;
  } = {
    turn_id: string(object["turn_id"], "live.turn_id"),
    index: u32(object["index"], "live.index"),
    text: string(object["text"], "live.text"),
  };
  if (object["reasoning"] !== undefined) {
    result.reasoning = string(object["reasoning"], "live.reasoning");
  }
  if (object["title"] !== undefined) {
    result.title = string(object["title"], "live.title");
  }
  return result;
}

function record(value: unknown, what: string): Json {
  if (!isJsonObject(value)) {
    throw new Invalid(`${what} is not an object`);
  }
  return value;
}

/** True for a JSON object: not null and not an array. */
export function isJsonObject(value: unknown): value is Json {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function string(value: unknown, what: string): string {
  if (typeof value !== "string") {
    throw new Invalid(`${what} is not a string`);
  }
  return value;
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
