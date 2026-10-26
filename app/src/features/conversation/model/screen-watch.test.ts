import { createStore } from "@tanstack/store";
import { describe, expect, it } from "vitest";

import type { Event, SubscribeItem } from "@/lib/protocol/wire";
import { EMPTY_STREAM, withLink, type StreamState } from "@/lib/stream/reducer";
import {
  approvalRequested,
  approvalResolved,
  envelope,
  eventItem,
  manualFrames,
  promptQueued,
  run,
  snapshotItem,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";
import type { Announcement } from "./announce";
import {
  approvalRowKey,
  failedTurns,
  followApprovalFocus,
  waitingApprovalKey,
  watchFocusedStream,
} from "./screen-watch";

let seq = 0;
const event = (body: Event): SubscribeItem => eventItem(++seq, body);
const apply = (state: StreamState, ...items: SubscribeItem[]) => run(items, state);

function failed(turnId: string): Event {
  return {
    kind: "turn_failed",
    turn_id: turnId,
    error: { code: "internal", message: "model error: overloaded" },
  };
}

/** A stream that ran one turn to its end: the first load of a screen. */
function loaded(): StreamState {
  return apply(
    EMPTY_STREAM,
    event(promptQueued("t1", "hi")),
    event(turnStarted("t1")),
    event(turnCompleted("t1")),
  );
}

describe("failedTurns", () => {
  it("names a turn that failed between two states", () => {
    const running = apply(loaded(), event(turnStarted("t2")));
    expect(failedTurns(running, apply(running, event(failed("t2"))))).toEqual(["t2"]);
  });

  it("names no turn that ended in another way, or that failed before", () => {
    const running = apply(loaded(), event(turnStarted("t2")));
    expect(failedTurns(running, apply(running, event(turnCompleted("t2"))))).toEqual([]);
    const history = apply(EMPTY_STREAM, event(turnStarted("t3")), event(failed("t3")));
    expect(failedTurns(EMPTY_STREAM, history)).toEqual([]);
    expect(failedTurns(history, history)).toEqual([]);
  });
});

function watch(initial: StreamState) {
  const store = createStore(initial);
  const said: Announcement[] = [];
  let haptics = 0;
  const stop = watchFocusedStream(store, {
    announce: (announcement) => said.push(announcement),
    failedTurn: () => {
      haptics += 1;
    },
  });
  return {
    store,
    said,
    haptics: () => haptics,
    stop,
    push: (...items: SubscribeItem[]) => store.setState((state) => apply(state, ...items)),
  };
}

describe("watchFocusedStream", () => {
  it("announces a question and the end of a turn, and plays the haptic of a failure", () => {
    const test = watch(loaded());
    test.push(event(turnStarted("t2")));
    test.push(event(approvalRequested("t2", "c1", "git push")));
    test.push(event(approvalResolved("t2", "c1", "allow")));
    test.push(event(turnCompleted("t2")));
    test.push(event(turnStarted("t3")));
    test.push(event(failed("t3")));
    expect(test.said.map((one) => one.text)).toEqual([
      "Approval needed. git push",
      "Reply done.",
      "The turn failed. model error: overloaded",
    ]);
    expect(test.haptics()).toBe(1);
  });

  it("is quiet for the first load of the stream", () => {
    const test = watch(EMPTY_STREAM);
    test.push(
      event(turnStarted("t1")),
      event(approvalRequested("t1", "c1", "git push")),
      event(failed("t1")),
    );
    expect(test.said).toEqual([]);
    expect(test.haptics()).toBe(0);
  });

  it("stops with the focus", () => {
    const test = watch(apply(loaded(), event(turnStarted("t2"))));
    test.stop();
    test.push(event(failed("t2")));
    expect(test.said).toEqual([]);
    expect(test.haptics()).toBe(0);
  });
});

function follow(initial: StreamState) {
  const store = createStore(initial);
  const frames = manualFrames();
  const scrolled: string[] = [];
  let done = 0;
  const stop = followApprovalFocus(store, {
    schedule: frames.schedule,
    scrollTo: (key) => scrolled.push(key),
    done: () => {
      done += 1;
    },
  });
  return { store, frames, scrolled, done: () => done, stop };
}

describe("followApprovalFocus", () => {
  it("scrolls to the approval that waits on the next frame, then ends", () => {
    const asked = apply(
      loaded(),
      event(turnStarted("t2")),
      event(approvalRequested("t2", "c1", "git push")),
    );
    expect(waitingApprovalKey(asked)).toBe(approvalRowKey("t2", "c1"));
    const test = follow(asked);
    expect(test.scrolled).toEqual([]);
    test.frames.frame();
    expect(test.scrolled).toEqual(["a:t2:c1"]);
    expect(test.done()).toBe(1);
  });

  it("waits for the stream to bring the approval", () => {
    const test = follow(EMPTY_STREAM);
    test.store.setState((state) => apply(withLink(state, "live"), event(turnStarted("t2"))));
    expect(test.done()).toBe(0);
    test.store.setState((state) => apply(state, event(approvalRequested("t2", "c1"))));
    test.frames.frame();
    expect(test.scrolled).toEqual(["a:t2:c1"]);
    expect(test.done()).toBe(1);
    test.store.setState((state) => apply(state, event(approvalRequested("t2", "c2"))));
    test.frames.frame();
    expect(test.scrolled).toEqual(["a:t2:c1"]);
  });

  it("finds the approval in a snapshot", () => {
    const test = follow(loaded());
    test.store.setState((state) =>
      apply(
        state,
        snapshotItem(
          "conversation",
          "awaiting_approval",
          [envelope(1, turnStarted("t2")), envelope(2, approvalRequested("t2", "c1", "git push"))],
          2,
        ),
      ),
    );
    test.frames.frame();
    expect(test.scrolled).toEqual(["a:t2:c1"]);
    expect(test.done()).toBe(1);
  });

  it("ends with no scroll when a snapshot has no approval that waits", () => {
    const test = follow(loaded());
    test.store.setState((state) => withLink(state, "live"));
    expect(test.done()).toBe(0);
    test.store.setState((state) =>
      apply(state, snapshotItem("conversation", "idle", [envelope(1, turnStarted("t1"))], 1)),
    );
    expect(test.done()).toBe(1);
    test.frames.frame();
    expect(test.scrolled).toEqual([]);
  });

  it("does not scroll after a stop", () => {
    const test = follow(
      apply(loaded(), event(turnStarted("t2")), event(approvalRequested("t2", "c1"))),
    );
    test.stop();
    test.frames.frame();
    expect(test.scrolled).toEqual([]);
    expect(test.done()).toBe(0);
  });
});
