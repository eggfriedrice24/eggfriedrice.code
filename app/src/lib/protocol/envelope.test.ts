/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";

import { EMPTY_STREAM, reduce } from "@/lib/stream/reducer";
import {
  decodeConversationSummary,
  decodeErrorBody,
  decodeEventEnvelope,
  decodeServerFrame,
  decodeSubscribeItem,
  isJsonObject,
} from "./envelope";
import { CALL, TURN } from "./testing";

// The frozen fixtures of efr-protocol: the same JSON that the Rust tests read.
const FIXTURES = "../../../../crates/efr-protocol/fixtures/v1";

const AT = "2026-10-03T12:00:05Z";

const frames = import.meta.glob<unknown>(
  "../../../../crates/efr-protocol/fixtures/v1/frames/*.json",
  {
    eager: true,
    import: "default",
  },
);
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
  it("decodes every event fixture to its own kind, and the unknown one to the skip wrapper", () => {
    const names = Object.keys(events);
    expect(names.length).toBeGreaterThan(40);
    for (const path of names) {
      const body = events[path];
      const wireKind = isJsonObject(body) ? String(body["kind"]) : "";
      const decoded = decodeEventEnvelope({ seq: 1, at: "2026-10-03T12:00:05Z", event: body });
      const kind = decoded.ok ? decoded.value.event.kind : "failed";
      const want = path.endsWith("/unknown.json") ? "other" : wireKind;
      expect({ path, kind }).toEqual({ path, kind: want });
    }
  });

  it("keeps every member of a known event and drops a member that it does not know", () => {
    const body = fixture(events, "approval_requested_local_only");
    const decoded = decodeEventEnvelope({ seq: 5, at: AT, event: body });
    expect(decoded).toEqual({ ok: true, value: { seq: 5, at: AT, event: body } });
    const newer = isJsonObject(body) ? { ...body, from_a_newer_efrd: 1 } : body;
    expect(decodeEventEnvelope({ seq: 5, at: AT, event: newer })).toEqual(decoded);
  });

  it("decodes an unknown kind to the skip wrapper, and a malformed known kind to an invalid one", () => {
    expect(
      decodeEventEnvelope({ seq: 2, at: AT, event: { kind: "from_the_future", x: 1 } }),
    ).toEqual({
      ok: true,
      value: { seq: 2, at: AT, event: { kind: "other", wireKind: "from_the_future" } },
    });
    expect(
      decodeEventEnvelope({
        seq: 3,
        at: AT,
        event: {
          kind: "assistant_message_updated",
          turn_id: TURN,
          index: 0,
          offset: "12",
          delta: "",
        },
      }),
    ).toEqual({
      ok: true,
      value: {
        seq: 3,
        at: AT,
        event: { kind: "other", wireKind: "assistant_message_updated", invalid: true },
      },
    });
  });

  it("refuses an envelope whose seq, time or conversation is malformed", () => {
    const event = { kind: "turn_started", turn_id: TURN };
    expect(decodeEventEnvelope({ seq: 2, event })).toMatchObject({ ok: false, reason: /^at/ });
    expect(decodeEventEnvelope({ seq: -2, at: AT, event })).toMatchObject({
      ok: false,
      reason: /^seq/,
    });
    for (const at of ["t", "2026-10-03", "2026-10-03T12:00Z", "2026-10-03T12:00:05"]) {
      expect({ at, ok: decodeEventEnvelope({ seq: 2, at, event }).ok }).toEqual({ at, ok: false });
    }
    for (const at of ["2026-10-03T12:00:05.5Z", "2026-10-03T12:00:05.123456789+02:00"]) {
      expect({ at, ok: decodeEventEnvelope({ seq: 2, at, event }).ok }).toEqual({ at, ok: true });
    }
    expect(
      decodeEventEnvelope({ seq: 2, at: AT, conversation_id: "not a uuid", event }),
    ).toMatchObject({ ok: false, reason: /^conversation_id/ });
    expect(decodeEventEnvelope({ seq: 2, at: AT, conversation_id: null, event })).toMatchObject({
      ok: true,
      value: { conversation_id: null },
    });
  });

  it("reads an id of version 0, as the tests of efrd make them", () => {
    const body = fixture(events, "turn_started");
    const turnId = "019a9b1c-3d00-0a10-0b20-000000000002";
    const event = isJsonObject(body) ? { ...body, turn_id: turnId } : body;
    expect(decodeEventEnvelope({ seq: 2, at: AT, event })).toMatchObject({
      ok: true,
      value: { event: { kind: "turn_started", turn_id: turnId } },
    });
  });

  it("decodes a known kind with an enum value from a newer efrd to an invalid wrapper", () => {
    const body = fixture(events, "tool_call_started");
    const launch = isJsonObject(body) ? { ...body, launch: { kind: "from_the_future" } } : body;
    expect(decodeEventEnvelope({ seq: 4, at: AT, event: launch })).toEqual({
      ok: true,
      value: {
        seq: 4,
        at: AT,
        event: { kind: "other", wireKind: "tool_call_started", invalid: true },
      },
    });
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
      decodeSubscribeItem({ kind: "draft", turn_id: TURN, after_seq: 1, draft: { kind: "image" } }),
    ).toEqual({
      ok: true,
      value: {
        kind: "draft",
        turn_id: TURN,
        after_seq: 1,
        draft: { kind: "other", wireKind: "image" },
      },
    });
  });

  it("decodes a malformed draft to an invalid wrapper, never to an error", () => {
    const part = { kind: "text", index: 0, offset: 0, delta: "a" };
    expect(
      decodeSubscribeItem({
        kind: "draft",
        turn_id: TURN,
        after_seq: 1,
        draft: { ...part, offset: -1 },
      }),
    ).toEqual({
      ok: true,
      value: {
        kind: "draft",
        turn_id: TURN,
        after_seq: 1,
        draft: { kind: "other", wireKind: "text", invalid: true },
      },
    });
    for (const item of [
      { kind: "draft", turn_id: "t", after_seq: 1, draft: part },
      { kind: "draft", turn_id: TURN, after_seq: "1", draft: part },
      { kind: "draft", turn_id: TURN, after_seq: 1, draft: "text" },
    ]) {
      expect(decodeSubscribeItem(item)).toEqual({
        ok: true,
        value: { kind: "other", wireKind: "draft", invalid: true },
      });
    }
  });

  it("refuses an item that is not an object or has no kind", () => {
    expect(decodeSubscribeItem(null)).toEqual({ ok: false, reason: "item is not an object" });
    expect(decodeSubscribeItem({ kind: 3 })).toEqual({
      ok: false,
      reason: "item.kind is not a string",
    });
  });

  it("decodes the live part of a snapshot", () => {
    const decoded = decodeSubscribeItem(
      fixture(items, "conversation_subscribe_item_snapshot_live"),
    );
    expect(decoded).toMatchObject({
      ok: true,
      value: {
        kind: "snapshot",
        hwm: 42,
        live: {
          index: 1,
          text: "The largest file is ~/日志/größe.log 🙂",
          text_bytes: 45,
          reasoning_title: "Checking the logs",
        },
      },
    });
    const state = decoded.ok ? reduce(EMPTY_STREAM, decoded.value) : EMPTY_STREAM;
    const turn = state.turns.get("019a9b1c-3d00-7a10-8b20-000000000002");
    expect(turn?.messages.get(1)?.buffer.bytes).toBe(45);
    expect(turn?.title).toBe("Checking the logs");
  });

  it("refuses a snapshot whose live part or conversation is malformed", () => {
    const base = fixture(items, "conversation_subscribe_item_snapshot_live");
    const snapshot = isJsonObject(base) ? base : {};
    const live = isJsonObject(snapshot["live"]) ? snapshot["live"] : {};
    expect(decodeSubscribeItem({ ...snapshot, live: { ...live, text_bytes: -1 } }).ok).toBe(false);
    expect(decodeSubscribeItem({ ...snapshot, conversation: { id: 1 } }).ok).toBe(false);
    expect(decodeSubscribeItem({ ...snapshot, live: null }).ok).toBe(true);
    const conversation = isJsonObject(snapshot["conversation"]) ? snapshot["conversation"] : {};
    expect(
      decodeSubscribeItem({ ...snapshot, conversation: { ...conversation, status: "paused" } }),
    ).toMatchObject({ ok: false, reason: /^snapshot\.conversation\.status/ });
  });

  it("keeps the events of a snapshot that it can read, and skips an invalid one", () => {
    const base = fixture(items, "conversation_subscribe_item_snapshot");
    const snapshot = isJsonObject(base) ? base : {};
    const bad = { seq: 41, at: AT, event: { kind: "turn_started", turn_id: 7 } };
    const decoded = decodeSubscribeItem({ ...snapshot, events: [bad] });
    expect(decoded).toMatchObject({
      ok: true,
      value: { events: [{ seq: 41, event: { kind: "other", invalid: true } }] },
    });
  });

  it("passes known draft parts that the reducer does not read", () => {
    expect(
      decodeSubscribeItem({
        kind: "draft",
        turn_id: TURN,
        after_seq: 1,
        draft: { kind: "compacting", trigger: "auto" },
      }),
    ).toMatchObject({ ok: true, value: { draft: { kind: "compacting", trigger: "auto" } } });
  });
});

describe("decodeErrorBody", () => {
  it("refuses a code that the protocol does not have and keeps data of any shape", () => {
    expect(decodeErrorBody({ code: "teapot", message: "x" })).toMatchObject({
      ok: false,
      reason: /^error\.code/,
    });
    expect(decodeErrorBody({ code: "busy", message: "x", data: 3 })).toEqual({
      ok: true,
      value: { code: "busy", message: "x", data: 3 },
    });
  });
});

describe("decodeConversationSummary", () => {
  it("decodes the summaries of a list result and refuses a malformed one", () => {
    const summary = {
      id: CALL,
      status: "idle",
      created_at: AT,
      updated_at: AT,
      last_seq: 3,
      title: null,
    };
    expect(decodeConversationSummary(summary)).toEqual({ ok: true, value: summary });
    expect(decodeConversationSummary({ ...summary, last_seq: 2 ** 53 })).toMatchObject({
      ok: false,
      reason: /^conversation\.last_seq/,
    });
  });
});
