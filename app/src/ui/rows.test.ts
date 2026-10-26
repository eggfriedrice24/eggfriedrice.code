import { describe, expect, it } from "vitest";

import { decodeSubscribeItem } from "../core/envelope";
import { EMPTY_STREAM, reduce, type StreamState } from "../core/reducer";
import { EMPTY_DETAILS, reduceDetails, type TranscriptDetails } from "./details";
import { createRowBuilder } from "./rows";

const TURN = "t1";

function feed(items: unknown[]): { details: TranscriptDetails; stream: StreamState } {
  let details = EMPTY_DETAILS;
  let stream = EMPTY_STREAM;
  for (const item of items) {
    details = reduceDetails(details, item);
    const decoded = decodeSubscribeItem(item);
    if (decoded.ok) {
      stream = reduce(stream, decoded.value);
    }
  }
  return { details, stream };
}

let seq = 0;
const event = (body: Record<string, unknown>) => ({
  kind: "event",
  seq: ++seq,
  at: "2026-10-26T12:00:00Z",
  event: { turn_id: TURN, ...body },
});
const text = (index: number, offset: number, delta: string) => ({
  kind: "draft",
  turn_id: TURN,
  after_seq: seq,
  draft: { kind: "text", index, offset, delta },
});

describe("createRowBuilder", () => {
  it("lists the parts of a turn newest first, with its status while it runs", () => {
    const build = createRowBuilder();
    const input = feed([
      event({ kind: "prompt_queued", text: "hi", origin: "phone" }),
      event({ kind: "turn_started" }),
      text(0, 0, "Hello"),
      event({ kind: "tool_call_started", call_id: "c", tool: "shell", input: { command: "ls" } }),
      event({ kind: "approval_requested", call_id: "c", summary: "run ls" }),
    ]);
    expect(build(input).map((row) => row.key)).toEqual([
      "w:t1",
      "a:t1:c",
      "t:t1:c",
      "m:t1:0",
      "p:t1",
    ]);
  });

  it("keeps the same list and rows while only the text grows", () => {
    const build = createRowBuilder();
    const before = feed([event({ kind: "turn_started" }), text(0, 0, "Hey")]);
    const rows = build(before);
    const after = { ...before, stream: reduceStream(before.stream, text(0, 3, " you")) };
    expect(after.stream).not.toBe(before.stream);
    expect(build(after)).toBe(rows);
  });

  it("keeps each row object when a row comes, and ends the status at the turn's end", () => {
    const build = createRowBuilder();
    const items: unknown[] = [event({ kind: "turn_started" }), text(0, 0, "Hi")];
    const first = build(feed(items));
    items.push(
      event({ kind: "assistant_message_completed", index: 0, text: "Hi" }),
      event({ kind: "turn_completed" }),
    );
    const second = build(feed(items));
    expect(second.map((row) => row.key)).toEqual(["e:t1", "m:t1:0"]);
    expect(second[1]).toBe(first.find((row) => row.key === "m:t1:0"));
  });

  it("shows the reasoning after the prompt, and no message before its first text", () => {
    const build = createRowBuilder();
    const rows = build(
      feed([
        event({ kind: "prompt_queued", text: "think", origin: "shell" }),
        event({ kind: "turn_started" }),
        {
          kind: "draft",
          turn_id: TURN,
          after_seq: seq,
          draft: { kind: "reasoning", offset: 0, delta: "**Plan**" },
        },
        text(0, 0, ""),
      ]),
    );
    expect(rows.map((row) => row.key)).toEqual(["w:t1", "r:t1", "p:t1"]);
  });
});

function reduceStream(stream: StreamState, item: unknown): StreamState {
  const decoded = decodeSubscribeItem(item);
  return decoded.ok ? reduce(stream, decoded.value) : stream;
}
