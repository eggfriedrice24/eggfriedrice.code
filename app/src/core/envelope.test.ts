/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";

import {
  decodeEventEnvelope,
  decodeServerFrame,
  decodeSubscribeItem,
  isJsonObject,
} from "./envelope";
import { EMPTY_STREAM, reduce } from "./reducer";

// The frozen fixtures of efr-protocol: the same JSON that the Rust tests read.
const FIXTURES = "../../../crates/efr-protocol/fixtures/v1";

const frames = import.meta.glob<unknown>("../../../crates/efr-protocol/fixtures/v1/frames/*.json", {
  eager: true,
  import: "default",
});
const events = import.meta.glob<unknown>("../../../crates/efr-protocol/fixtures/v1/events/*.json", {
  eager: true,
  import: "default",
});
const items = import.meta.glob<unknown>(
  "../../../crates/efr-protocol/fixtures/v1/conversation_subscribe_item_*.json",
  { eager: true, import: "default" },
);

function fixture(table: Record<string, unknown>, name: string): unknown {
  const path = Object.keys(table).find((key) => key.endsWith(`/${name}.json`));
  if (path === undefined) {
    throw new Error(`no fixture ${name} under ${FIXTURES}`);
  }
  return table[path];
}

describe("decodeServerFrame", () => {
  it("decodes every server frame fixture", () => {
    expect(decodeServerFrame(JSON.stringify(fixture(frames, "server_item")))).toMatchObject({
      ok: true,
      value: { type: "item", id: 7 },
    });
    expect(decodeServerFrame(JSON.stringify(fixture(frames, "server_end")))).toEqual({
      ok: true,
      value: { type: "end", id: 7 },
    });
    expect(decodeServerFrame(JSON.stringify(fixture(frames, "server_ack")))).toEqual({
      ok: true,
      value: { type: "ack", id: 7 },
    });
    expect(decodeServerFrame(JSON.stringify(fixture(frames, "server_error")))).toEqual({
      ok: true,
      value: {
        type: "error",
        id: 7,
        error: {
          code: "overflow",
          message: "the subscriber fell behind; resume after seq 41",
          data: { last_seq: 41 },
        },
      },
    });
    expect(
      decodeServerFrame(JSON.stringify(fixture(frames, "server_error_without_id"))),
    ).toMatchObject({ ok: true, value: { type: "error", id: null } });
  });

  it("refuses client frames and malformed envelopes", () => {
    for (const name of ["client_cancel", "client_request"]) {
      expect(decodeServerFrame(JSON.stringify(fixture(frames, name))).ok).toBe(false);
    }
    for (const text of [
      "not json",
      "[]",
      "null",
      '{"id": 1}',
      '{"id": 1, "item": {}, "end": true}',
      '{"id": 1, "end": false}',
      '{"id": -1, "end": true}',
      '{"id": 1.5, "end": true}',
      '{"id": 9007199254740993, "end": true}',
      '{"item": {}}',
      '{"id": 1, "error": {"code": 3, "message": "x"}}',
    ]) {
      expect({ text, ok: decodeServerFrame(text).ok }).toEqual({ text, ok: false });
    }
  });
});

describe("decodeEventEnvelope", () => {
  it("decodes every event fixture, the ones the reducer reads with their fields", () => {
    const read = new Set([
      "turn_started",
      "assistant_message_updated",
      "assistant_message_completed",
      "tool_call_started",
      "tool_call_output_updated",
      "tool_call_completed",
      "turn_completed",
      "turn_failed",
      "turn_interrupted",
      "turn_cancelled",
    ]);
    const names = Object.keys(events);
    expect(names.length).toBeGreaterThan(40);
    for (const path of names) {
      const body = events[path];
      const wireKind = isJsonObject(body) ? String(body["kind"]) : "";
      const decoded = decodeEventEnvelope({ seq: 1, at: "2026-10-03T12:00:05Z", event: body });
      const kind = decoded.ok ? decoded.value.event.kind : "failed";
      expect({ path, kind }).toEqual({ path, kind: read.has(wireKind) ? wireKind : "other" });
    }
  });

  it("decodes an unknown kind to the skip wrapper and refuses a malformed known kind", () => {
    expect(
      decodeEventEnvelope({ seq: 2, at: "t", event: { kind: "from_the_future", x: 1 } }),
    ).toEqual({
      ok: true,
      value: { seq: 2, at: "t", event: { kind: "other", wireKind: "from_the_future" } },
    });
    expect(
      decodeEventEnvelope({
        seq: 2,
        at: "t",
        event: {
          kind: "assistant_message_updated",
          turn_id: "t",
          index: 0,
          offset: "12",
          delta: "",
        },
      }).ok,
    ).toBe(false);
    expect(decodeEventEnvelope({ seq: 2, event: { kind: "turn_started", turn_id: "t" } }).ok).toBe(
      false,
    );
  });
});

describe("decodeSubscribeItem", () => {
  it("decodes the subscribe item fixtures and feeds the reducer", () => {
    let state = EMPTY_STREAM;
    for (const name of [
      "conversation_subscribe_item_snapshot",
      "conversation_subscribe_item_event",
      "conversation_subscribe_item_draft",
    ]) {
      const decoded = decodeSubscribeItem(fixture(items, name));
      expect({ name, ok: decoded.ok }).toEqual({ name, ok: true });
      if (decoded.ok) {
        state = reduce(state, decoded.value);
      }
    }
    expect(state.lastSeq).toBeGreaterThanOrEqual(42);
    expect(state.turns.size).toBe(1);
  });

  it("decodes an unknown item kind and an unknown draft kind to skip wrappers", () => {
    expect(decodeSubscribeItem({ kind: "presence" })).toEqual({
      ok: true,
      value: { kind: "other", wireKind: "presence" },
    });
    expect(
      decodeSubscribeItem({ kind: "draft", turn_id: "t", after_seq: 1, draft: { kind: "image" } }),
    ).toEqual({
      ok: true,
      value: {
        kind: "draft",
        turn_id: "t",
        after_seq: 1,
        draft: { kind: "other", wireKind: "image" },
      },
    });
  });

  it("decodes the live part of a snapshot", () => {
    const decoded = decodeSubscribeItem({
      kind: "snapshot",
      events: [],
      hwm: 3,
      live: { turn_id: "t", index: 0, text: "so far", reasoning: "r", title: "T" },
    });
    expect(decoded).toMatchObject({
      ok: true,
      value: { live: { turn_id: "t", index: 0, text: "so far", reasoning: "r", title: "T" } },
    });
  });
});
