import { describe, expect, it } from "vitest";

import type { ApprovalView } from "@/lib/stream/reducer";
import {
  answeredWhere,
  answerRefusal,
  approvalCard,
  localReason,
  nextTick,
  phoneDecisions,
} from "./approval";

const BASE: ApprovalView = {
  callId: "c",
  turnId: "t",
  summary: "run `rm -rf build`",
  diffPreview: undefined,
  reason: undefined,
  modelReason: undefined,
  exitKinds: [],
  always: undefined,
  localOnly: false,
  interactive: false,
  askedAt: 0,
  expiresAt: undefined,
  status: "pending",
  decision: undefined,
  answeredBy: undefined,
  closedAt: undefined,
};

const online = { connected: true, sending: undefined, error: undefined };

describe("approvalCard", () => {
  it("offers deny and allow for a pending question, never always", () => {
    const card = approvalCard(BASE, 0, online);
    expect(card.title).toBe("Approval needed");
    expect(card.buttons.map((button) => [button.decision, button.label, button.enabled])).toEqual([
      ["deny", "Deny", true],
      ["allow", "Allow", true],
    ]);
    expect(card.countdown).toBeUndefined();
    expect(card.label).toBe("Approval needed: run `rm -rf build`");
  });

  it("gives no buttons for a question that only the computer may answer, and says why", () => {
    const privilege = approvalCard(
      { ...BASE, localOnly: true, exitKinds: ["privilege"] },
      0,
      online,
    );
    expect(privilege.title).toBe("Answer on your computer");
    expect(privilege.buttons).toEqual([]);
    expect(privilege.notes).toEqual(["It needs your password, which only the terminal can take."]);
    expect(privilege.label).toBe("Approval needed, answer on your computer: run `rm -rf build`");
    const job = approvalCard({ ...BASE, localOnly: true }, 0, online);
    expect(job.notes[0]).toContain("keeps running after the turn ends");
  });

  it("says that an always rule is set only in the terminal", () => {
    const card = approvalCard({ ...BASE, always: "host crates.io:443" }, 0, online);
    expect(card.notes).toEqual(['An "always" rule can only be set in the terminal.']);
    expect(card.buttons).toHaveLength(2);
  });

  it("counts down a fixed limit and closes the buttons at its end", () => {
    const asked = { ...BASE, expiresAt: 120_000 };
    expect(approvalCard(asked, 15_000, online).countdown).toBe("Expires in 1:45");
    const over = approvalCard(asked, 120_000, online);
    expect(over.countdown).toBe("The time to answer is over");
    expect(over.buttons.every((button) => !button.enabled)).toBe(true);
  });

  it("closes the buttons while offline or while an answer is on its way", () => {
    const offline = approvalCard(BASE, 0, { ...online, connected: false });
    expect(offline.buttons.every((button) => !button.enabled)).toBe(true);
    expect(offline.notes).toContain(
      "You can answer when the phone connects to your computer again.",
    );
    const sending = approvalCard(BASE, 0, { ...online, sending: "allow" });
    expect(sending.buttons.map((button) => button.label)).toEqual(["Deny", "Allowing"]);
    expect(sending.buttons.every((button) => !button.enabled)).toBe(true);
  });

  it("shows the error of a failed answer", () => {
    const card = approvalCard(BASE, 0, { ...online, error: "efrd refused the answer." });
    expect(card.notes).toContain("efrd refused the answer.");
  });

  it("says who answered", () => {
    const allowed = approvalCard(
      { ...BASE, status: "allowed", decision: "allow", answeredBy: "phone" },
      0,
      online,
    );
    expect([allowed.title, allowed.tone, allowed.notes]).toEqual([
      "Allowed",
      "success",
      ["Answered on a phone."],
    ]);
    const denied = approvalCard(
      { ...BASE, status: "denied", decision: "deny", answeredBy: "shell" },
      0,
      online,
    );
    expect([denied.title, denied.tone, denied.notes]).toEqual([
      "Denied",
      "error",
      ["Answered in a terminal."],
    ]);
    const always = approvalCard(
      { ...BASE, status: "allowed", decision: "allow_always" },
      0,
      online,
    );
    expect(always.title).toBe("Allowed always");
    expect(always.buttons).toEqual([]);
  });

  it("tells an expiry, an expiry while away and a settled question apart, in clear words", () => {
    const expired = approvalCard({ ...BASE, status: "expired" }, 0, online);
    expect(expired.title).toBe("Expired");
    expect(expired.notes[0]).toContain("The call did not run.");
    const away = approvalCard({ ...BASE, status: "expired_away" }, 0, online);
    expect(away.title).toBe("Expired while you were away");
    expect(away.notes[0]).toBe("Nobody answered while you were away. The call did not run.");
    const settled = approvalCard({ ...BASE, status: "settled" }, 0, online);
    expect([settled.title, settled.buttons]).toEqual(["Closed", []]);
  });
});

describe("the answers of the phone", () => {
  it("offers allow and deny only for a pending question that the phone may answer", () => {
    expect(phoneDecisions(BASE)).toEqual(["allow", "deny"]);
    expect(phoneDecisions({ ...BASE, localOnly: true })).toEqual([]);
    expect(phoneDecisions({ ...BASE, status: "denied" })).toEqual([]);
  });

  it("refuses an answer that efrd would refuse", () => {
    expect(answerRefusal(undefined, "allow")).toBe("unknown");
    expect(answerRefusal({ ...BASE, status: "expired" }, "allow")).toBe("not_pending");
    expect(answerRefusal({ ...BASE, localOnly: true }, "deny")).toBe("local_only");
    expect(answerRefusal(BASE, "allow_always")).toBe("decision");
    expect(answerRefusal(BASE, "allow")).toBeUndefined();
  });

  it("names why a question is local only", () => {
    expect(localReason(BASE)).toBeUndefined();
    expect(localReason({ ...BASE, localOnly: true, interactive: true })).toBe("privilege");
    expect(localReason({ ...BASE, localOnly: true, expiresAt: 5 })).toBe("delete");
    expect(localReason({ ...BASE, localOnly: true })).toBe("job");
  });

  it("names where an answer came from", () => {
    expect(answeredWhere("cli")).toBe("in a terminal");
    expect(answeredWhere(undefined)).toBeUndefined();
  });
});

describe("nextTick", () => {
  it("wakes the block when the shown second changes", () => {
    const asked = { ...BASE, expiresAt: 10_500 };
    expect(nextTick(asked, 0)).toBe(500);
    expect(nextTick(asked, 500)).toBe(1_500);
    expect(nextTick(asked, 10_500)).toBeUndefined();
    expect(nextTick(BASE, 0)).toBeUndefined();
    expect(nextTick({ ...asked, status: "expired" }, 0)).toBeUndefined();
  });
});
