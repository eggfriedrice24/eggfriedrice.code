/// <reference types="vite/client" />
import { describe, expect, it } from "vitest";

import {
  decodeErrorBody,
  decodeEventEnvelope,
  decodeServerFrame,
  decodeSubscribeItem,
  isJsonObject,
} from "./envelope";
import { EMPTY_STREAM, reduce } from "@/lib/stream/reducer";

// The frozen fixtures of efr-protocol: the same JSON that the Rust tests read.
const FIXTURES = "../../../../crates/efr-protocol/fixtures/v1";

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

  it("keeps every member of a known event", () => {
    const body = fixture(events, "approval_requested_local_only");
    const decoded = decodeEventEnvelope({ seq: 5, at: "t", event: body });
    expect(decoded).toEqual({ ok: true, value: { seq: 5, at: "t", event: body } });
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
  });

  it("passes known draft parts that the reducer does not read", () => {
    expect(
      decodeSubscribeItem({
        kind: "draft",
        turn_id: "t",
        after_seq: 1,
        draft: { kind: "compacting", trigger: "auto" },
      }),
    ).toMatchObject({ ok: true, value: { draft: { kind: "compacting", trigger: "auto" } } });
  });
});

describe("decodeErrorBody", () => {
  it("refuses a code that the protocol does not have and drops data that is not an object", () => {
    expect(decodeErrorBody({ code: "teapot", message: "x" }).ok).toBe(false);
    expect(decodeErrorBody({ code: "busy", message: "x", data: 3 })).toEqual({
      ok: true,
      value: { code: "busy", message: "x" },
    });
  });
});
