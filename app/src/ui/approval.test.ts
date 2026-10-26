import { describe, expect, it } from "vitest";

import { approvalCard, nextTick } from "./approval";
import type { ApprovalView } from "./details";

const BASE: ApprovalView = {
  callId: "c",
  turnId: "t",
  summary: "run `rm -rf build`",
  interactive: false,
  localOnly: false,
  state: "pending",
  away: false,
};

const online = { connected: true };

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

  it("gives no buttons for a question that only the computer may answer", () => {
    const card = approvalCard({ ...BASE, localOnly: true }, 0, online);
    expect(card.title).toBe("Answer on your computer");
    expect(card.buttons).toEqual([]);
    expect(card.notes[0]).toContain("Only your computer can answer");
  });

  it("counts down a fixed limit and closes the buttons at its end", () => {
    const asked = { ...BASE, expiresAt: 120_000 };
    expect(approvalCard(asked, 15_000, online).countdown).toBe("Expires in 1:45");
    const over = approvalCard(asked, 120_000, online);
    expect(over.countdown).toBe("The time to answer is over");
    expect(over.buttons.every((button) => !button.enabled)).toBe(true);
  });

  it("closes the buttons while offline or while an answer is on its way", () => {
    const offline = approvalCard(BASE, 0, { connected: false });
    expect(offline.buttons.every((button) => !button.enabled)).toBe(true);
    expect(offline.notes).toContain(
      "You can answer when the phone connects to your computer again.",
    );
    const sending = approvalCard(BASE, 0, { connected: true, sending: "allow" });
    expect(sending.buttons.map((button) => button.label)).toEqual(["Deny", "Allowing"]);
    expect(sending.buttons.every((button) => !button.enabled)).toBe(true);
  });

  it("says that a command can ask for input that the phone cannot type", () => {
    const card = approvalCard({ ...BASE, interactive: true }, 0, online);
    expect(card.notes[0]).toContain("You cannot type it on the phone");
  });

  it("shows the error of a failed answer", () => {
    const card = approvalCard(BASE, 0, { connected: true, error: "efrd refused the answer." });
    expect(card.notes).toContain("efrd refused the answer.");
  });

  it("says who answered", () => {
    const allowed = approvalCard(
      { ...BASE, state: "resolved", decision: "allow", answeredBy: "phone" },
      0,
      online,
    );
    expect([allowed.title, allowed.tone, allowed.notes]).toEqual([
      "Allowed",
      "success",
      ["Answered on a phone."],
    ]);
    const denied = approvalCard(
      { ...BASE, state: "resolved", decision: "deny", answeredBy: "shell" },
      0,
      online,
    );
    expect([denied.title, denied.tone, denied.notes]).toEqual([
      "Denied",
      "error",
      ["Answered on your computer."],
    ]);
    const always = approvalCard(
      { ...BASE, state: "resolved", decision: "allow_always" },
      0,
      online,
    );
    expect(always.title).toBe("Allowed always");
    expect(always.buttons).toEqual([]);
  });

  it("tells an expiry from an expiry while away, in clear words", () => {
    const expired = approvalCard({ ...BASE, state: "expired" }, 0, online);
    expect(expired.title).toBe("Expired");
    expect(expired.notes[0]).toContain("The call did not run.");
    const away = approvalCard({ ...BASE, state: "expired", away: true }, 0, online);
    expect(away.title).toBe("Expired while you were away");
    expect(away.notes[0]).toBe("Nobody answered while you were away. The call did not run.");
  });
});

describe("nextTick", () => {
  it("wakes the card when the shown second changes", () => {
    const asked = { ...BASE, expiresAt: 10_500 };
    expect(nextTick(asked, 0)).toBe(500);
    expect(nextTick(asked, 500)).toBe(1_500);
    expect(nextTick(asked, 10_500)).toBeUndefined();
    expect(nextTick(BASE, 0)).toBeUndefined();
    expect(nextTick({ ...asked, state: "expired" }, 0)).toBeUndefined();
  });
});
