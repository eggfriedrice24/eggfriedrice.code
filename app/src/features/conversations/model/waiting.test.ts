import { describe, expect, it } from "vitest";

import { item } from "./testing";
import { approvalsElsewhere, manyApprovalsText, showsApprovalDot } from "./waiting";

describe("approvalsElsewhere", () => {
  it("is none when nothing waits", () => {
    const result = approvalsElsewhere({
      current: undefined,
      live: [{ machine: "d", id: "a", pendingApprovals: 0 }],
      listed: [item("d", "b", 1, { status: "running" })],
    });
    expect(result).toEqual({ kind: "none" });
    expect(showsApprovalDot(result)).toBe(false);
  });

  it("gives one waiting conversation from a polled list", () => {
    const result = approvalsElsewhere({
      current: { machine: "d", id: "here" },
      live: [],
      listed: [
        item("d", "here", 9, { status: "awaiting_approval" }),
        item("l", "steer", 5, { status: "awaiting_approval", title: "Fix steer race" }),
      ],
    });
    expect(result).toEqual({
      kind: "one",
      conversation: { machine: "l", id: "steer", title: "Fix steer race" },
    });
    expect(showsApprovalDot(result)).toBe(true);
  });

  it("never counts the conversation on screen", () => {
    const result = approvalsElsewhere({
      current: { machine: "d", id: "a" },
      live: [{ machine: "d", id: "a", pendingApprovals: 2 }],
      listed: [item("d", "a", 1, { status: "awaiting_approval" })],
    });
    expect(result).toEqual({ kind: "none" });
  });

  it("counts the same id on another machine as another conversation", () => {
    const result = approvalsElsewhere({
      current: { machine: "d", id: "a" },
      live: [],
      listed: [item("l", "a", 1, { status: "awaiting_approval" })],
    });
    expect(result.kind).toBe("one");
  });

  it("trusts a live stream over a polled list that is behind", () => {
    const answered = approvalsElsewhere({
      current: undefined,
      live: [{ machine: "d", id: "a", pendingApprovals: 0 }],
      listed: [item("d", "a", 1, { status: "awaiting_approval" })],
    });
    expect(answered).toEqual({ kind: "none" });
    const asked = approvalsElsewhere({
      current: undefined,
      live: [{ machine: "d", id: "a", title: "Live title", pendingApprovals: 1 }],
      listed: [item("d", "a", 1, { status: "running", title: "Old title" })],
    });
    expect(asked).toEqual({
      kind: "one",
      conversation: { machine: "d", id: "a", title: "Live title" },
    });
  });

  it("counts conversations, the polled ones newest first, then the live ones they lack", () => {
    const result = approvalsElsewhere({
      current: { machine: "d", id: "here" },
      live: [
        { machine: "d", id: "stacked", pendingApprovals: 3 },
        { machine: "d", id: "old", pendingApprovals: 1 },
      ],
      listed: [
        item("d", "old", 1, { status: "awaiting_approval" }),
        item("l", "new", 7, { status: "awaiting_approval" }),
        item("l", "new", 7, { status: "awaiting_approval" }),
      ],
    });
    expect(result).toEqual({
      kind: "many",
      count: 3,
      conversations: [
        { machine: "l", id: "new" },
        { machine: "d", id: "old" },
        { machine: "d", id: "stacked" },
      ],
    });
    expect(manyApprovalsText(3)).toBe("3 approvals waiting");
  });
});
