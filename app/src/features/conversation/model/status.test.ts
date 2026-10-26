import { describe, expect, it } from "vitest";

import type { DraftPart, Event, SubscribeItem } from "@/lib/protocol/wire";
import type { TurnEnd } from "@/lib/stream/reducer";
import {
  approvalRequested,
  draftItem,
  eventItem,
  promptQueued,
  run,
  toolCallStarted,
  turnStarted,
} from "@/lib/stream/testing";
import { endLine, turnStatus } from "./status";

const TURN = "t";
let seq = 0;
const event = (body: Event): SubscribeItem => eventItem(++seq, body);
const draft = (part: DraftPart): SubscribeItem => draftItem(TURN, seq, part);

function statusAfter(...items: SubscribeItem[]): string {
  const state = run(items);
  return turnStatus(state.turns.get(TURN), state).text;
}

function end(fields: Partial<TurnEnd> & Pick<TurnEnd, "outcome">): TurnEnd {
  return {
    at: undefined,
    context: undefined,
    error: undefined,
    stop: undefined,
    usage: undefined,
    ...fields,
  };
}

describe("turnStatus", () => {
  it("names what the running turn does now", () => {
    expect(statusAfter(event(promptQueued(TURN, "x", "phone")))).toBe("waiting in the queue");
    expect(statusAfter(event(turnStarted(TURN)))).toBe("working");
    expect(
      statusAfter(
        event(turnStarted(TURN)),
        draft({ kind: "reasoning", offset: 0, delta: "**Plan**", title: "Plan" }),
      ),
    ).toBe("thinking: Plan");
    expect(
      statusAfter(
        event(turnStarted(TURN)),
        draft({ kind: "text", index: 0, offset: 0, delta: "a" }),
      ),
    ).toBe("writing");
    expect(
      statusAfter(
        event(turnStarted(TURN)),
        draft({ kind: "tool_input", call: 0, tool: "apply_patch", bytes: 2_399 }),
      ),
    ).toBe("writing apply_patch 2.3 KB");
    expect(statusAfter(event(turnStarted(TURN)), event(toolCallStarted(TURN, "c", "shell")))).toBe(
      "running shell",
    );
    expect(
      statusAfter(
        event(turnStarted(TURN)),
        event({ ...approvalRequested(TURN, "c"), local_only: true }),
      ),
    ).toBe("waiting for you on your computer");
    expect(
      statusAfter(event(turnStarted(TURN)), draft({ kind: "compacting", trigger: "auto" })),
    ).toBe("compacting context");
  });

  it("carries the context gauge of the conversation", () => {
    const state = run([
      event(turnStarted(TURN)),
      draft({ kind: "context", tokens: 89_400, limit: 206_720, window: 0 }),
    ]);
    expect(turnStatus(state.turns.get(TURN), state).gauge?.text).toBe("ctx 43%");
  });
});

describe("endLine", () => {
  it("says how the turn ended, with the gauge when it has a count", () => {
    expect(
      endLine(
        end({ outcome: "completed", context: { tokens: 89_400, limit: 206_720, window: 0 } }),
      ),
    ).toMatchObject({
      text: "done",
      tone: "muted",
      gauge: { text: "ctx 43%" },
      detail: "(89k of 206k tokens)",
    });
    expect(endLine(end({ outcome: "failed", error: "The provider failed." }))).toEqual({
      text: "failed: The provider failed.",
      tone: "error",
      gauge: undefined,
      detail: undefined,
    });
    expect(endLine(end({ outcome: "interrupted" })).text).toBe("stopped");
    expect(endLine(end({ outcome: "completed", stop: "max_tokens" })).text).toBe(
      "done: the model stopped (max_tokens)",
    );
    expect(endLine(end({ outcome: "cancelled" })).tone).toBe("error");
    expect(endLine(end({ outcome: "withdrawn" })).text).toBe("withdrawn: the prompt did not run");
  });
});
