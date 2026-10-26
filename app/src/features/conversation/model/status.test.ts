import { describe, expect, it } from "vitest";

import { decodeSubscribeItem } from "@/lib/protocol/envelope";
import { EMPTY_STREAM, reduce, type StreamState } from "@/lib/stream/reducer";
import { EMPTY_DETAILS, reduceDetails, type TranscriptDetails } from "./details";
import { endLine, turnStatus } from "./status";

const TURN = "t";
let seq = 0;
const event = (body: Record<string, unknown>) => ({
  kind: "event",
  seq: ++seq,
  at: "2026-10-26T12:00:00Z",
  event: { turn_id: TURN, ...body },
});
const draft = (part: Record<string, unknown>) => ({
  kind: "draft",
  turn_id: TURN,
  after_seq: seq,
  draft: part,
});

function statusAfter(...items: unknown[]): string {
  let details: TranscriptDetails = EMPTY_DETAILS;
  let stream: StreamState = EMPTY_STREAM;
  for (const item of items) {
    details = reduceDetails(details, item);
    const decoded = decodeSubscribeItem(item);
    if (decoded.ok) {
      stream = reduce(stream, decoded.value);
    }
  }
  return turnStatus(stream.turns.get(TURN), details.turns.get(TURN), details).text;
}

describe("turnStatus", () => {
  it("names what the running turn does now", () => {
    expect(statusAfter(event({ kind: "prompt_queued", text: "x", origin: "phone" }))).toBe(
      "waiting in the queue",
    );
    expect(statusAfter(event({ kind: "turn_started" }))).toBe("working");
    expect(
      statusAfter(
        event({ kind: "turn_started" }),
        draft({ kind: "reasoning", offset: 0, delta: "**Plan**", title: "Plan" }),
      ),
    ).toBe("thinking: Plan");
    expect(
      statusAfter(
        event({ kind: "turn_started" }),
        draft({ kind: "text", index: 0, offset: 0, delta: "a" }),
      ),
    ).toBe("writing");
    expect(
      statusAfter(
        event({ kind: "turn_started" }),
        draft({ kind: "tool_input", call: 0, tool: "apply_patch", bytes: 2_399 }),
      ),
    ).toBe("writing apply_patch 2.3 KB");
    expect(
      statusAfter(
        event({ kind: "turn_started" }),
        event({ kind: "tool_call_started", call_id: "c", tool: "shell", input: {} }),
      ),
    ).toBe("running shell");
    expect(
      statusAfter(
        event({ kind: "turn_started" }),
        event({ kind: "approval_requested", call_id: "c", summary: "x", local_only: true }),
      ),
    ).toBe("waiting for you on your computer");
    expect(
      statusAfter(event({ kind: "turn_started" }), draft({ kind: "compacting", trigger: "auto" })),
    ).toBe("compacting context");
  });
});

describe("endLine", () => {
  it("says how the turn ended, with the gauge when it has a count", () => {
    expect(
      endLine({ outcome: "completed", context: { tokens: 89_400, limit: 206_720, window: 0 } }),
    ).toMatchObject({
      text: "done",
      tone: "muted",
      gauge: { text: "ctx 43%" },
      detail: "(89k of 206k tokens)",
    });
    expect(endLine({ outcome: "failed", error: "The provider failed." })).toEqual({
      text: "failed: The provider failed.",
      tone: "error",
    });
    expect(endLine({ outcome: "interrupted" }).text).toBe("stopped");
    expect(endLine({ outcome: "completed", stop: "max_tokens" }).text).toBe(
      "done: the model stopped (max_tokens)",
    );
    expect(endLine({ outcome: "withdrawn" }).text).toBe("withdrawn: the prompt did not run");
  });
});
