import { describe, expect, it } from "vitest";

import type { Attention, ConversationStatus } from "./generated";
import { rowState, waitsForUser, type RowState } from "./status";

type Case = readonly [ConversationStatus, Attention | null | undefined, boolean, RowState];

// The table of spec 0.0.12, section 6.1, in its order.
const CASES: readonly Case[] = [
  ["awaiting_approval", undefined, true, "approval"],
  ["awaiting_approval", null, false, "approval"],
  ["awaiting_approval", "answer", true, "approval"],
  ["awaiting_approval", "unknown", true, "approval"],
  ["awaiting_approval", "terminal", true, "terminal"],
  ["running", "answer", true, "answer"],
  ["running", "answer", false, "terminal"],
  ["running", "terminal", true, "terminal"],
  ["running", undefined, true, "running"],
  ["running", "unknown", true, "running"],
  ["running", "failed", true, "running"],
  ["idle", "failed", true, "failed"],
  ["idle", "unknown", true, "idle"],
  ["idle", null, true, "idle"],
  ["idle", undefined, false, "idle"],
];

describe("rowState", () => {
  it.each(CASES)("reads %s with %s (can answer: %s) as %s", (status, attention, can, state) => {
    expect(rowState(status, attention, can)).toBe(state);
  });
});

describe("waitsForUser", () => {
  it("is true for the three waits only", () => {
    const states: readonly RowState[] = [
      "approval",
      "answer",
      "terminal",
      "running",
      "failed",
      "idle",
    ];
    expect(states.filter(waitsForUser)).toEqual(["approval", "answer", "terminal"]);
  });
});
