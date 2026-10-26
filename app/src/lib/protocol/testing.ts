/// <reference types="vite/client" />
// Helpers for the tests of the store: the event fixtures of efr-protocol as envelopes and
// subscribe items. Only test files import this module.

import { decodeEventEnvelope, decodeSubscribeItem } from "./envelope";
import type { EventEnvelope, SubscribeItem } from "./wire";

const events = import.meta.glob<unknown>(
  "../../../../crates/efr-protocol/fixtures/v1/events/*.json",
  {
    eager: true,
    import: "default",
  },
);

const items = import.meta.glob<unknown>(
  "../../../../crates/efr-protocol/fixtures/v1/conversation_subscribe_item_*.json",
  { eager: true, import: "default" },
);

export const CONVERSATION = "019a9b1c-3d00-7a10-8b20-000000000001";
export const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";
export const CALL = "019a9b1c-3d00-7a10-8b20-000000000004";

function find(table: Record<string, unknown>, name: string): unknown {
  const path = Object.keys(table).find((key) => key.endsWith(`/${name}.json`));
  if (path === undefined) {
    throw new Error(`no fixture ${name}`);
  }
  return table[path];
}

/** The raw event item of fixture `events/<name>.json` at sequence number `seq`. */
export function rawEvent(name: string, seq: number, overrides: object = {}): unknown {
  const body = find(events, name);
  return {
    kind: "event",
    seq,
    at: "2026-10-26T12:00:00Z",
    conversation_id: CONVERSATION,
    event: typeof body === "object" && body !== null ? { ...body, ...overrides } : body,
  };
}

/** The event of fixture `events/<name>.json` as an envelope at sequence number `seq`. */
export function fixtureEvent(name: string, seq: number, overrides: object = {}): EventEnvelope {
  const decoded = decodeEventEnvelope(rawEvent(name, seq, overrides));
  if (!decoded.ok) {
    throw new Error(`fixture ${name} does not decode: ${decoded.reason}`);
  }
  return decoded.value;
}

/** Decodes a raw subscribe item, or throws. */
export function item(raw: unknown): SubscribeItem {
  const decoded = decodeSubscribeItem(raw);
  if (!decoded.ok) {
    throw new Error(`the item does not decode: ${decoded.reason}`);
  }
  return decoded.value;
}

/** The raw subscribe item fixture `conversation_subscribe_item_<name>.json`. */
export function rawItem(name: string): unknown {
  return find(items, `conversation_subscribe_item_${name}`);
}
