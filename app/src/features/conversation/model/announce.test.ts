import { describe, expect, it } from "vitest";

import type { Event, SubscribeItem } from "@/lib/protocol/wire";
import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import {
  approvalRequested,
  eventItem,
  run,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";
import { announcements } from "./announce";

let seq = 0;
const event = (body: Event): SubscribeItem => eventItem(++seq, body);
const apply = (state: StreamState, ...items: SubscribeItem[]) => run(items, state);

describe("announcements", () => {
  it("announces a new question at once and the end of a turn in the queue", () => {
    const running = apply(EMPTY_STREAM, event(turnStarted("t")));
    const asked = apply(running, event(approvalRequested("t", "c", "run ls")));
    expect(announcements(running, asked)).toEqual([
      { text: "Approval needed. run ls", priority: "high", queue: false },
    ]);
    expect(announcements(asked, asked)).toEqual([]);
    const done = apply(asked, event(turnCompleted("t")));
    expect(announcements(asked, done)).toEqual([
      { text: "Reply done.", priority: "low", queue: true },
    ]);
  });

  it("says where to answer a local question", () => {
    const asked = apply(
      EMPTY_STREAM,
      event({ ...approvalRequested("t", "c", "sudo"), local_only: true }),
    );
    expect(announcements(EMPTY_STREAM, asked)[0]?.text).toBe(
      "Approval needed on your computer. sudo",
    );
  });

  it("does not announce the end of a turn that it never saw run", () => {
    const history = apply(EMPTY_STREAM, event(turnStarted("t")), event(turnCompleted("t")));
    expect(announcements(EMPTY_STREAM, history)).toEqual([]);
  });

  it("names a failed turn with its error", () => {
    const running = apply(EMPTY_STREAM, event(turnStarted("t")));
    const failed = apply(
      running,
      event({
        kind: "turn_failed",
        turn_id: "t",
        error: { code: "internal", message: "The provider failed." },
      }),
    );
    expect(announcements(running, failed)[0]?.text).toBe("The turn failed. The provider failed.");
  });
});
