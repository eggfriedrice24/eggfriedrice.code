import { describe, expect, it } from "vitest";

import { createDemoSession, type DemoTimers } from "./demo_session";
import { pendingApprovals } from "./details";
import { APPROVAL_CALL, DEMO_APPROVAL_TURN, DEMO_CONVERSATION } from "./scenarios";

/** A fake clock: `run(ms)` moves time on and fires each timer that comes due, in order. */
function fakeTimers(start = 1_000_000) {
  let now = start;
  let id = 0;
  let queue: { id: number; at: number; callback: () => void }[] = [];
  const timers: DemoTimers = {
    schedule: (callback, ms) => {
      id += 1;
      const entry = { id, at: now + ms, callback };
      queue.push(entry);
      return () => {
        queue = queue.filter((other) => other !== entry);
      };
    },
    now: () => now,
  };
  const run = (ms: number) => {
    const end = now + ms;
    for (;;) {
      const due = [...queue].sort((a, b) => a.at - b.at || a.id - b.id)[0];
      if (due === undefined || due.at > end) {
        break;
      }
      queue = queue.filter((entry) => entry !== due);
      now = due.at;
      due.callback();
    }
    now = end;
  };
  return { timers, run, pending: () => queue.length };
}

const HOUR = 3_600_000;

describe("the demo session", () => {
  it("plays the script up to the question, then waits for the answer", async () => {
    const clock = fakeTimers();
    const session = createDemoSession(clock.timers);
    const source = session.conversation(DEMO_CONVERSATION);
    clock.run(HOUR);
    const pending = pendingApprovals(source.details.getSnapshot());
    expect(pending.map((approval) => approval.callId)).toEqual([APPROVAL_CALL]);
    expect(pending[0]?.expiresAt).toBe((pending[0]?.askedAt ?? 0) + 120_000);
    expect(session.conversations.getSnapshot()[0]?.status).toBe("awaiting_approval");
    expect(clock.pending()).toBe(0);

    await expect(
      session.answerApproval(DEMO_CONVERSATION, APPROVAL_CALL, "allow"),
    ).resolves.toEqual({ ok: true });
    clock.run(HOUR);
    const turn = source.details.getSnapshot().turns.get(DEMO_APPROVAL_TURN);
    expect(turn?.approvals.get(APPROVAL_CALL)).toMatchObject({
      state: "resolved",
      decision: "allow",
    });
    expect(turn?.end?.outcome).toBe("completed");
    const message = source.stream.getSnapshot().turns.get(DEMO_APPROVAL_TURN)?.messages.get(2);
    expect(message?.buffer.text).toBe("Done. The journal now takes 500 MB.");
    expect(message?.final).toBe(true);
    expect(session.conversations.getSnapshot()[0]?.status).toBe("idle");
  });

  it("answers a prompt from the phone with a scripted reply", async () => {
    const clock = fakeTimers();
    const session = createDemoSession(clock.timers);
    const source = session.conversation(DEMO_CONVERSATION);
    clock.run(HOUR);
    await session.answerApproval(DEMO_CONVERSATION, APPROVAL_CALL, "deny");
    clock.run(HOUR);
    await expect(session.sendPrompt(DEMO_CONVERSATION, "hello")).resolves.toEqual({ ok: true });
    clock.run(HOUR);
    const details = source.details.getSnapshot();
    const last = details.turns.get(details.order[details.order.length - 1] ?? "");
    expect(last?.prompt).toEqual({ text: "hello", origin: "phone" });
    expect(last?.end?.outcome).toBe("completed");
    expect(await session.answerApproval(DEMO_CONVERSATION, APPROVAL_CALL, "allow")).toMatchObject({
      ok: false,
    });
    session.dispose();
  });
});
