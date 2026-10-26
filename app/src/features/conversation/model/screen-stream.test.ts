import { createStore, type Store } from "@tanstack/store";
import { describe, expect, it } from "vitest";

import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import {
  approvalRequested,
  eventItem,
  manualFrames,
  run,
  snapshotItem,
  turnCompleted,
  turnStarted,
} from "@/lib/stream/testing";
import type { ConversationRef } from "@/features/machines/model/machine-ref";
import type { HoldMode, StreamHold } from "@/features/machines/model/machine-supervisor";
import type { SupervisedStream } from "@/features/machines/model/stream-factory";
import type { Announcement } from "./announce";
import { ScreenStream, type HoldRequest, type StreamSource } from "./screen-stream";

const DESKTOP = "019a9b1c-3d00-7a10-8b20-0000000000d1";
const ONE: ConversationRef = { machine: DESKTOP, id: "019a9b1c-3d00-7a10-8b20-0000000000a1" };
const TWO: ConversationRef = { machine: DESKTOP, id: "019a9b1c-3d00-7a10-8b20-0000000000a2" };

interface FakeStream extends SupervisedStream {
  readonly store: Store<StreamState>;
}

/** A stream that a test feeds through its store. */
function stream(): FakeStream {
  return {
    store: createStore(EMPTY_STREAM),
    hydrated: Promise.resolve(),
    dispose: () => undefined,
    item: () => undefined,
    status: () => undefined,
    cursor: () => 0,
  };
}

/** A source that records each hold, and knows only the machine desktop. */
class FakeSource implements StreamSource<FakeStream> {
  readonly holds: { key: string; mode: HoldMode; released: number }[] = [];
  readonly streams = new Map<string, FakeStream>();

  hold(ref: ConversationRef, mode: HoldMode): StreamHold<FakeStream> | undefined {
    if (ref.machine !== DESKTOP) {
      return undefined;
    }
    let held = this.streams.get(ref.id);
    if (held === undefined) {
      held = stream();
      this.streams.set(ref.id, held);
    }
    const record = { key: ref.id, mode, released: 0 };
    this.holds.push(record);
    return {
      stream: held,
      release: () => {
        record.released += 1;
      },
    };
  }
}

/** A screen over a fake source, and what its effects did. */
function screenOf() {
  const source = new FakeSource();
  const frames = manualFrames();
  const said: Announcement[] = [];
  const scrolled: string[] = [];
  const done: ConversationRef[] = [];
  let failed = 0;
  const screen = new ScreenStream(source, {
    announce: (announcement) => said.push(announcement),
    failedTurn: () => {
      failed += 1;
    },
    schedule: frames.schedule,
    scrollTo: (key) => scrolled.push(key),
    focusDone: (ref) => done.push(ref),
  });
  return { source, frames, said, scrolled, done, failed: () => failed, screen };
}

function request(overrides: Partial<HoldRequest> = {}): HoldRequest {
  return { machine: DESKTOP, id: ONE.id, focused: true, focus: undefined, ...overrides };
}

describe("the hold of a screen", () => {
  it("has the stream while it holds it, and gives the hold back once", () => {
    const { source, screen } = screenOf();
    expect(screen.stream(ONE)).toBeUndefined();
    const release = screen.hold(ONE, "follow");
    expect(screen.stream(ONE)).toBeDefined();
    expect(screen.stream(TWO)).toBeUndefined();
    release();
    release();
    expect(screen.stream(ONE)).toBeUndefined();
    expect(source.holds).toEqual([{ key: ONE.id, mode: "follow", released: 1 }]);
  });

  it("keeps the new hold when the old one goes after it, as React resubscribes", () => {
    const { source, screen } = screenOf();
    const follow = screen.hold(ONE, "follow");
    const keep = screen.hold(ONE, "keep");
    follow();
    expect(screen.stream(ONE)).toBeDefined();
    keep();
    expect(screen.stream(ONE)).toBeUndefined();
    expect(source.holds.map((hold) => [hold.mode, hold.released])).toEqual([
      ["follow", 1],
      ["keep", 1],
    ]);
  });

  it("has no stream for a machine that is not paired", () => {
    const { screen } = screenOf();
    const other = { machine: "019a9b1c-3d00-7a10-8b20-0000000000d2", id: ONE.id };
    const release = screen.hold(other, "follow");
    expect(screen.stream(other)).toBeUndefined();
    release();
  });
});

describe("the subscriber of a screen", () => {
  it("gives the same function for the same request, so a render keeps the hold", () => {
    const { screen } = screenOf();
    const first = screen.subscriber(request());
    expect(screen.subscriber(request())).toBe(first);
    expect(screen.subscriber(request({ focused: false }))).not.toBe(first);
    expect(screen.subscriber(request({ focus: "approval" }))).not.toBe(first);
    expect(screen.subscriber(request({ id: TWO.id }))).not.toBe(first);
  });

  it("follows with the focus and keeps without it, and tells React of the stream", () => {
    const { source, screen } = screenOf();
    let changes = 0;
    const stop = screen.subscriber(request())(() => {
      changes += 1;
    });
    expect(changes).toBe(1);
    expect(screen.stream(ONE)).toBe(source.streams.get(ONE.id));
    stop();
    screen.subscriber(request({ focused: false }))(() => undefined)();
    expect(source.holds.map((hold) => [hold.mode, hold.released])).toEqual([
      ["follow", 1],
      ["keep", 1],
    ]);
  });

  it("takes no hold for a conversation that efrd did not make yet", () => {
    const { source, screen } = screenOf();
    screen.subscriber(request({ id: undefined }))(() => undefined)();
    expect(source.holds).toEqual([]);
  });

  it("watches the stream for news only while the screen has the focus", () => {
    const { source, screen, said, failed } = screenOf();
    const stop = screen.subscriber(request())(() => undefined);
    const held = source.streams.get(ONE.id);
    if (held === undefined) {
      throw new Error("no stream");
    }
    held.store.setState((state) =>
      run([eventItem(1, turnStarted("t1")), eventItem(2, turnCompleted("t1"))], state),
    );
    held.store.setState((state) => run([eventItem(3, turnStarted("t2"))], state));
    held.store.setState((state) =>
      run(
        [
          eventItem(4, {
            kind: "turn_failed",
            turn_id: "t2",
            error: { code: "internal", message: "no" },
          }),
        ],
        state,
      ),
    );
    expect(said.map((one) => one.text)).toEqual(["The turn failed. no"]);
    expect(failed()).toBe(1);
    stop();

    const unfocused = screen.subscriber(request({ focused: false }))(() => undefined);
    held.store.setState((state) =>
      run([eventItem(5, turnStarted("t3")), eventItem(6, approvalRequested("t3", "c1"))], state),
    );
    expect(said).toHaveLength(1);
    unfocused();
  });

  it("scrolls to the approval of a focus request, then clears the request", () => {
    const { source, screen, frames, scrolled, done } = screenOf();
    const stop = screen.subscriber(request({ focus: "approval" }))(() => undefined);
    source.streams.get(ONE.id)?.store.setState((state) =>
      run(
        [
          snapshotItem(
            ONE.id,
            "awaiting_approval",
            [
              { seq: 1, at: "2026-10-26T12:00:00Z", event: turnStarted("t1") },
              { seq: 2, at: "2026-10-26T12:00:00Z", event: approvalRequested("t1", "c1") },
            ],
            2,
          ),
        ],
        state,
      ),
    );
    frames.frame();
    expect(scrolled).toEqual(["a:t1:c1"]);
    expect(done).toEqual([ONE]);
    stop();
  });
});
