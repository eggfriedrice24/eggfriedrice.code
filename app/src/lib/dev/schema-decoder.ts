// The decoder of the app (src/lib/protocol/envelope.ts) again, with a parse option for
// each schema call, for the jitless mode of the decode bench (plan section 2.3, item 12).
//
// zod reads `z.config({ jitless })` when it makes an object schema, so the setting cannot
// change after the schemas exist. A parse with `{ jitless: true }` skips the compiled
// parser of an object for that call only. The bench gives this decoder that option, so
// one launch of the app measures both modes. A test checks that it decodes every item
// of the recordings to the same value as the app's decoder. It logs nothing. Pure.

import type { z } from "zod";

import { DRAFT_PART_KINDS, EVENT_KINDS, type ConversationSummary } from "@/lib/protocol/generated";
import {
  conversationSnapshotSchema,
  draftPartSchema,
  draftSchema,
  eventEnvelopeSchema,
  eventSchema,
} from "@/lib/protocol/generated-zod";
import { failureReason, isJsonObject, type Decoded } from "@/lib/protocol/envelope";
import type {
  Draft,
  DraftPart,
  Event,
  EventEnvelope,
  LiveTurn,
  OtherKind,
  Snapshot,
  SubscribeItem,
} from "@/lib/protocol/wire";

/** The parse option of every schema call. */
export interface SchemaParseOptions {
  /** True skips the compiled object parsers of zod. */
  readonly jitless: boolean;
}

type Json = Readonly<Record<string, unknown>>;

/** A failed check of a member that must be valid. */
class Invalid extends Error {}

const EVENT_KIND_SET: ReadonlySet<string> = new Set<string>(EVENT_KINDS);
const DRAFT_PART_KIND_SET: ReadonlySet<string> = new Set<string>(DRAFT_PART_KINDS);

const ENVELOPE = eventEnvelopeSchema.shape;
const DRAFT = draftSchema.shape;
const SNAPSHOT = conversationSnapshotSchema.shape;

/** A decoder of subscribe items that parses with `options`. */
export function schemaDecoder(
  options: SchemaParseOptions,
): (raw: unknown) => Decoded<SubscribeItem> {
  const check = <T>(schema: z.ZodType<T>, value: unknown, what: string): T => {
    const result = schema.safeParse(value, options);
    if (!result.success) {
      throw new Invalid(failureReason(what, result.error));
    }
    return result.data;
  };

  const eventEnvelope = (value: unknown): EventEnvelope => {
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
  };

  const event = (value: unknown): Event => {
    const object = record(value, "event");
    const kind = kindOf(object, "event");
    if (!EVENT_KIND_SET.has(kind)) {
      return { kind: "other", wireKind: kind };
    }
    const result = eventSchema.safeParse(object, options);
    return result.success ? result.data : invalidKind(kind);
  };

  const draftPart = (object: Json): DraftPart => {
    const kind = kindOf(object, "draft.draft");
    if (!DRAFT_PART_KIND_SET.has(kind)) {
      return { kind: "other", wireKind: kind };
    }
    const result = draftPartSchema.safeParse(object, options);
    return result.success ? result.data : invalidKind(kind);
  };

  const draftItem = (object: Json): SubscribeItem => {
    const turnId = DRAFT.turn_id.safeParse(object["turn_id"], options);
    const afterSeq = DRAFT.after_seq.safeParse(object["after_seq"], options);
    const part = object["draft"];
    if (!turnId.success || !afterSeq.success) {
      return invalidKind("draft");
    }
    if (!isJsonObject(part) || typeof part["kind"] !== "string") {
      return invalidKind("draft");
    }
    const draft: Draft = { turn_id: turnId.data, after_seq: afterSeq.data, draft: draftPart(part) };
    return { kind: "draft", ...draft };
  };

  const snapshot = (object: Json): Snapshot => {
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
  };

  const subscribeItem = (value: unknown): SubscribeItem => {
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
  };

  return (raw) => {
    try {
      return { ok: true, value: subscribeItem(raw) };
    } catch (error) {
      if (error instanceof Invalid) {
        return { ok: false, reason: error.message };
      }
      throw error;
    }
  };
}

function invalidKind(wireKind: string): OtherKind {
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
