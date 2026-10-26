import { describe, expect, it } from "vitest";

import { announcements } from "./announce";
import { EMPTY_DETAILS, reduceDetails, type TranscriptDetails } from "./details";

let seq = 0;
const event = (body: Record<string, unknown>) => ({
  kind: "event",
  seq: ++seq,
  at: "2026-10-26T12:00:00Z",
  event: { turn_id: "t", ...body },
});

const apply = (state: TranscriptDetails, ...items: unknown[]) => items.reduce(reduceDetails, state);

describe("announcements", () => {
  it("announces a new question at once and the end of a turn in the queue", () => {
    const running = apply(EMPTY_DETAILS, event({ kind: "turn_started" }));
    const asked = apply(
      running,
      event({ kind: "approval_requested", call_id: "c", summary: "run ls" }),
    );
    expect(announcements(running, asked)).toEqual([
      { text: "Approval needed. run ls", priority: "high", queue: false },
    ]);
    expect(announcements(asked, asked)).toEqual([]);
    const done = apply(asked, event({ kind: "turn_completed" }));
    expect(announcements(asked, done)).toEqual([
      { text: "Reply done.", priority: "low", queue: true },
    ]);
  });

  it("says where to answer a local question", () => {
    const asked = apply(
      EMPTY_DETAILS,
      event({ kind: "approval_requested", call_id: "c", summary: "sudo", local_only: true }),
    );
    expect(announcements(EMPTY_DETAILS, asked)[0]?.text).toBe(
      "Approval needed on your computer. sudo",
    );
  });

  it("does not announce the end of a turn that it never saw run", () => {
    const history = apply(
      EMPTY_DETAILS,
      event({ kind: "turn_started" }),
      event({ kind: "turn_completed" }),
    );
    expect(announcements(EMPTY_DETAILS, history)).toEqual([]);
  });

  it("names a failed turn with its error", () => {
    const running = apply(EMPTY_DETAILS, event({ kind: "turn_started" }));
    const failed = apply(
      running,
      event({ kind: "turn_failed", error: { code: "internal", message: "The provider failed." } }),
    );
    expect(announcements(running, failed)[0]?.text).toBe("The turn failed. The provider failed.");
  });
});
