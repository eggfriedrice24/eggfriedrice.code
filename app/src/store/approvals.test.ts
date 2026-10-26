import { describe, expect, it } from "vitest";

import {
  EMPTY_APPROVALS,
  applyApprovalEvent,
  applyApprovalSnapshot,
  expiresAt,
  pendingApprovals,
  phoneDecisions,
  type ApprovalsState,
} from "./approvals";
import { CALL, CONVERSATION, TURN, fixtureEvent } from "@/lib/protocol/testing";

function run(...names: [string, object?][]): ApprovalsState {
  let state = EMPTY_APPROVALS;
  names.forEach(([name, overrides], index) => {
    state = applyApprovalEvent(state, fixtureEvent(name, index + 1, overrides));
  });
  return state;
}

function summary(status: "idle" | "running" | "awaiting_approval") {
  return {
    id: CONVERSATION,
    status,
    created_at: "2026-10-26T11:00:00Z",
    updated_at: "2026-10-26T12:00:00Z",
    last_seq: 50,
  };
}

describe("approvals", () => {
  it("holds a question as pending with its facts", () => {
    const state = run(["approval_requested"]);
    expect(state.byCall.get(CALL)).toEqual({
      callId: CALL,
      turnId: TURN,
      summary: "run `journalctl --vacuum-size=500M` as root",
      diffPreview:
        "--- a/etc/systemd/journald.conf\n+++ b/etc/systemd/journald.conf\n-#SystemMaxUse=\n+SystemMaxUse=500M\n",
      localOnly: false,
      interactive: true,
      requestedAt: "2026-10-26T12:00:00Z",
      expiresInSecs: 120,
      status: "pending",
      decidedBy: undefined,
    });
    expect(pendingApprovals(state).map((view) => view.callId)).toEqual([CALL]);
    const view = state.byCall.get(CALL);
    expect(view && phoneDecisions(view)).toEqual(["allow", "deny"]);
    expect(view && expiresAt(view)).toBe(Date.parse("2026-10-26T12:02:00Z"));
  });

  it("never offers an answer for a local-only question", () => {
    const view = run(["approval_requested_local_only"]).byCall.get(CALL);
    expect(view?.localOnly).toBe(true);
    expect(view && phoneDecisions(view)).toEqual([]);
    expect(view && expiresAt(view)).toBeUndefined();
  });

  it("records each answer and who gave it", () => {
    for (const [decision, status] of [
      ["deny", "denied"],
      ["allow", "allowed"],
      ["allow_always", "allowed_always"],
    ] as const) {
      const state = run(
        ["approval_requested"],
        ["approval_resolved", { decision, origin: "shell" }],
      );
      expect(state.byCall.get(CALL)).toMatchObject({ status, decidedBy: "shell" });
      expect(pendingApprovals(state)).toEqual([]);
      const view = state.byCall.get(CALL);
      expect(view && phoneDecisions(view)).toEqual([]);
    }
  });

  it("tells an expiry from an expiry while away, and an answer stays", () => {
    expect(run(["approval_requested"], ["approval_expired"]).byCall.get(CALL)?.status).toBe(
      "expired_away",
    );
    expect(
      run(["approval_requested"], ["approval_expired", { away: false }]).byCall.get(CALL)?.status,
    ).toBe("expired");
    expect(
      run(["approval_requested"], ["approval_resolved"], ["approval_expired"]).byCall.get(CALL)
        ?.status,
    ).toBe("denied");
  });

  it("ignores duplicates, other kinds and answers to questions it never saw", () => {
    const state = run(["approval_requested"]);
    expect(applyApprovalEvent(state, fixtureEvent("approval_resolved", 1))).toBe(state);
    const other = applyApprovalEvent(state, fixtureEvent("turn_started", 5));
    expect(other.byCall).toBe(state.byCall);
    expect(other.lastSeq).toBe(5);
    expect(run(["approval_resolved"]).byCall.size).toBe(0);
  });

  it("rebuilds from a snapshot and settles what the window misses", () => {
    const held = run(["approval_requested"]);
    const waiting = applyApprovalSnapshot(held, {
      events: [],
      hwm: 40,
      conversation: summary("awaiting_approval"),
    });
    expect(waiting.byCall.get(CALL)?.status).toBe("pending");
    expect(waiting.lastSeq).toBe(40);

    const idle = applyApprovalSnapshot(held, {
      events: [],
      hwm: 40,
      conversation: summary("idle"),
    });
    expect(idle.byCall.get(CALL)?.status).toBe("settled");

    const answered = applyApprovalSnapshot(held, {
      events: [fixtureEvent("approval_requested", 30), fixtureEvent("approval_resolved", 31)],
      hwm: 40,
      conversation: summary("idle"),
    });
    expect(answered.byCall.get(CALL)).toMatchObject({ status: "denied", decidedBy: "phone" });
    expect(answered.order).toEqual([CALL]);
  });
});
