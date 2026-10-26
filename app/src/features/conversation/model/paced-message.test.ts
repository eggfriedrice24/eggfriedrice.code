import { createStore } from "@tanstack/store";
import { describe, expect, it } from "vitest";

import { EMPTY_STREAM, type StreamState } from "@/lib/stream/reducer";
import {
  draftItem,
  eventItem,
  manualFrames,
  messageCompleted,
  promptQueued,
  run,
  textPart,
  turnStarted,
} from "@/lib/stream/testing";
import { createPacedMessage, type PacedMessageDeps } from "./paced-message";

const TURN = "019a9b1c-3d00-7a10-8b20-000000000002";

function started(): StreamState {
  return run([eventItem(1, promptQueued(TURN, "Fix it.")), eventItem(2, turnStarted(TURN))]);
}

function withText(state: StreamState, offset: number, delta: string): StreamState {
  return run([draftItem(TURN, 2, textPart(0, offset, delta))], state);
}

function setup(initial: StreamState, reduce = false) {
  const store = createStore(initial);
  const frames = manualFrames();
  let now = 0;
  const deps: PacedMessageDeps = {
    schedule: frames.schedule,
    now: () => now,
    reduceMotion: () => reduce,
  };
  const paced = createPacedMessage(store, TURN, 0, deps);
  let renders = 0;
  const stop = paced.subscribe(() => renders++);
  return {
    store,
    frames,
    paced,
    stop,
    renders: () => renders,
    advance(ms: number) {
      now += ms;
      frames.frame();
    },
  };
}

describe("createPacedMessage", () => {
  it("shows nothing before the message exists", () => {
    const { paced } = setup(EMPTY_STREAM);
    expect(paced.snapshot()).toMatchObject({ message: undefined, text: "", streaming: false });
  });

  it("reveals new text over frames and never moves back", () => {
    const view = setup(started());
    view.store.setState((state) => withText(state, 0, "Found it. The steer lands late."));
    const lengths: number[] = [];
    for (let i = 0; i < 20; i++) {
      view.advance(16);
      lengths.push(view.paced.snapshot().text.length);
    }
    expect(lengths[0]).toBeLessThan("Found it. The steer lands late.".length);
    expect(lengths.at(-1)).toBe("Found it. The steer lands late.".length);
    for (let i = 1; i < lengths.length; i++) {
      expect(lengths[i]).toBeGreaterThanOrEqual(lengths[i - 1] ?? 0);
    }
    expect(view.paced.snapshot().streaming).toBe(true);
  });

  it("notifies at most once per frame, and only when the text changes", () => {
    const view = setup(withText(started(), 0, "Hello"));
    const before = view.renders();
    view.advance(16);
    view.advance(16);
    expect(view.renders()).toBe(before);
    view.store.setState((state) => withText(state, 5, " there"));
    view.store.setState((state) => withText(state, 11, ", again"));
    view.advance(16);
    expect(view.renders()).toBeLessThanOrEqual(before + 1);
  });

  it("asks for no frames once the final text shows", () => {
    const view = setup(withText(started(), 0, "Done"));
    view.store.setState((state) => run([eventItem(3, messageCompleted(TURN, 0, "Done."))], state));
    view.advance(16);
    expect(view.paced.snapshot()).toMatchObject({ text: "Done.", streaming: false });
    expect(view.frames.pending()).toBe(0);
  });

  it("shows the whole text on each frame with Reduce Motion", () => {
    const view = setup(started(), true);
    view.store.setState((state) => withText(state, 0, "All at once, please."));
    view.advance(16);
    expect(view.paced.snapshot().text).toBe("All at once, please.");
  });

  it("splits a code fence into its own segment as it streams", () => {
    const view = setup(started(), true);
    view.store.setState((state) => withText(state, 0, "Guard it:\n\n```rust\nfn a("));
    view.advance(16);
    expect(view.paced.snapshot().segments.map((segment) => segment.kind)).toEqual([
      "markdown",
      "code",
    ]);
  });

  it("stops following the store after the last unsubscribe", () => {
    const view = setup(started());
    view.stop();
    view.store.setState((state) => withText(state, 0, "late"));
    expect(view.frames.pending()).toBe(0);
    expect(view.paced.snapshot().text).toBe("");
  });

  it("continues from what it showed when it subscribes again", () => {
    const view = setup(withText(started(), 0, "Some text"), true);
    view.advance(16);
    view.stop();
    view.store.setState((state) => withText(state, 9, " more"));
    const stop = view.paced.subscribe(() => undefined);
    view.advance(16);
    expect(view.paced.snapshot().text).toBe("Some text more");
    stop();
  });
});
