import { describe, expect, it } from "vitest";

import type { Event } from "@/lib/protocol/generated";
import { DemoLog } from "./demo-log";

const FACTS = { id: "c1", title: "demo", cwd: "/home/me", createdAt: 0 };

/** A log after `events`, each sent one ms after the one before. */
function logOf(events: readonly Event[]): DemoLog {
  const log = new DemoLog(FACTS);
  events.forEach((event, index) => log.send({ kind: "event", wait: 0, event }, index + 1));
  return log;
}

function state(log: DemoLog): readonly [string, string | undefined] {
  const summary = log.summary();
  return [summary.status, summary.attention ?? undefined];
}

function started(turn: string): Event {
  return { kind: "turn_started", turn_id: turn, cwd: "/home/me", scope: { kind: "machine" } };
}

function input(callId: string, wait: "none" | "visible" | "hidden"): Event {
  return { kind: "tool_call_input_changed", turn_id: "t1", call_id: callId, input: wait };
}

function approval(callId: string, localOnly: boolean): Event {
  return {
    kind: "approval_requested",
    turn_id: "t1",
    call_id: callId,
    summary: "sudo btrfs subvolume delete",
    interactive: false,
    local_only: localOnly,
  };
}

describe("DemoLog attention (spec 0.0.12, section 6.2)", () => {
  it("gives answer while a question of the model waits", () => {
    const asked: Event = {
      kind: "user_question_requested",
      turn_id: "t1",
      call_id: "k1",
      question_id: "q1",
      question: "Which database?",
      options: [{ label: "Postgres" }, { label: "SQLite" }],
    };
    const answered: Event = {
      kind: "user_question_answered",
      turn_id: "t1",
      question_id: "q1",
      answer: { kind: "replied", reply: { kind: "option", index: 0 } },
    };
    expect(state(logOf([started("t1"), asked]))).toEqual(["running", "answer"]);
    expect(state(logOf([started("t1"), asked, answered]))).toEqual(["running", undefined]);
  });

  it("gives terminal for a local-only approval or an input wait, none with another approval", () => {
    const local = approval("k1", true);
    expect(state(logOf([started("t1"), local]))).toEqual(["awaiting_approval", "terminal"]);
    expect(state(logOf([started("t1"), local, approval("k2", false)]))).toEqual([
      "awaiting_approval",
      undefined,
    ]);
    expect(state(logOf([started("t1"), input("k3", "hidden")]))).toEqual(["running", "terminal"]);
    expect(state(logOf([started("t1"), input("k3", "hidden"), input("k3", "none")]))).toEqual([
      "running",
      undefined,
    ]);
  });

  it("gives failed after a failed last turn, until the next one starts", () => {
    const failed: Event = {
      kind: "turn_failed",
      turn_id: "t1",
      error: { code: "internal", message: "500" },
    };
    const done: Event = { kind: "turn_completed", turn_id: "t2" };
    expect(state(logOf([started("t1"), failed]))).toEqual(["idle", "failed"]);
    expect(state(logOf([started("t1"), failed, started("t2")]))).toEqual(["running", undefined]);
    expect(state(logOf([started("t1"), failed, started("t2"), done]))).toEqual(["idle", undefined]);
  });
});
