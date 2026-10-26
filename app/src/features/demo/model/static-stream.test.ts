import { describe, expect, it } from "vitest";

import type { StreamState } from "@/lib/stream/reducer";
import { approvalCases, demoConversation } from "./demo-conversations";
import { runningTurnOf, staticState } from "./static-stream";

const END = Date.parse("2026-10-26T12:00:00Z");

function ids(): () => string {
  let n = 0;
  return () => {
    n += 1;
    return `0199d3e0-de35-7000-8000-${n.toString(16).padStart(12, "0")}`;
  };
}

function turns(state: StreamState) {
  return state.order.map((id) => state.turns.get(id));
}

describe("the stream state of a sheet", () => {
  it("holds every turn end of the messages sheet and a queued prompt", () => {
    const state = staticState(demoConversation("messages", ids()).history, END);
    expect(state.loaded).toBe(true);
    expect(turns(state).map((turn) => turn?.end?.outcome)).toEqual([
      "completed",
      "interrupted",
      "failed",
      undefined,
      undefined,
    ]);
    expect(turns(state)[3]?.title).toBe("Running the tests");
    expect(turns(state)[4]?.started).toBe(false);
    expect(runningTurnOf(state)).toBe(state.order[3]);
  });

  it("holds a running call and a patch that the model writes on the tools sheet", () => {
    const state = staticState(demoConversation("tools", ids()).history, END);
    const turn = turns(state)[0];
    expect(turn?.tools.size).toBe(5);
    expect([...(turn?.tools.values() ?? [])].filter((call) => !call.done)).toHaveLength(1);
    expect(turn?.toolInputs.get(1)?.tool).toBe("apply_patch");
    expect(runningTurnOf(state)).toBe(state.order[0]);
  });

  it("ends at the time it is given", () => {
    const state = staticState(demoConversation("diff", ids()).history, END);
    expect(turns(state)[0]?.end?.at).toBe(END);
  });

  it("gives each approval case its status", () => {
    const statuses = approvalCases(ids()).map((one) => {
      const state = staticState(one.steps, END);
      return state.turns.get(one.turnId)?.approvals.get(one.callId)?.status;
    });
    expect(statuses).toEqual([
      "pending",
      "pending",
      "pending",
      "pending",
      "pending",
      "pending",
      "allowed",
      "denied",
      "pending",
      "pending",
      "pending",
      "expired",
      "expired_away",
    ]);
  });
});
