import { describe, expect, it } from "vitest";

import { combineStores, createValueStore, frameGated, memoSelector } from "./frame-store";

/** A manual frame clock: `frame()` runs the callbacks of the next frame. */
function manualFrames() {
  let queue: (() => void)[] = [];
  return {
    schedule: (callback: () => void) => {
      queue.push(callback);
      return () => {
        queue = queue.filter((other) => other !== callback);
      };
    },
    frame: () => {
      const now = queue;
      queue = [];
      for (const callback of now) {
        callback();
      }
    },
    pending: () => queue.length,
  };
}

describe("createValueStore", () => {
  it("tells each listener of a change, and nobody of the same value", () => {
    const store = createValueStore(1);
    let calls = 0;
    const off = store.subscribe(() => calls++);
    store.set(1);
    expect(calls).toBe(0);
    store.update((value) => value + 1);
    expect(store.getSnapshot()).toBe(2);
    expect(calls).toBe(1);
    off();
    store.set(3);
    expect(calls).toBe(1);
  });
});

describe("frameGated", () => {
  it("tells its listeners once per frame, with the last value of the frame", () => {
    const source = createValueStore(0);
    const frames = manualFrames();
    const gated = frameGated(source, frames.schedule);
    const seen: number[] = [];
    gated.subscribe(() => seen.push(gated.getSnapshot()));
    for (let i = 1; i <= 50; i++) {
      source.set(i);
    }
    expect(gated.getSnapshot()).toBe(0);
    expect(frames.pending()).toBe(1);
    frames.frame();
    expect(seen).toEqual([50]);
    frames.frame();
    expect(seen).toEqual([50]);
  });

  it("stops following the source without listeners, and reads it on each call then", () => {
    const source = createValueStore("a");
    const frames = manualFrames();
    const gated = frameGated(source, frames.schedule);
    const off = gated.subscribe(() => {});
    source.set("b");
    off();
    expect(frames.pending()).toBe(0);
    expect(gated.getSnapshot()).toBe("b");
    source.set("c");
    expect(gated.getSnapshot()).toBe("c");
  });

  it("takes the value at once on a flush", () => {
    const source = createValueStore(0);
    const frames = manualFrames();
    const gated = frameGated(source, frames.schedule);
    let calls = 0;
    gated.subscribe(() => calls++);
    source.set(1);
    gated.flush();
    expect(gated.getSnapshot()).toBe(1);
    expect(calls).toBe(1);
    expect(frames.pending()).toBe(0);
  });
});

describe("memoSelector", () => {
  it("returns the last result while the input or the result stays the same", () => {
    let runs = 0;
    const select = memoSelector(
      (values: readonly number[]) => {
        runs++;
        return values.filter((value) => value > 1);
      },
      (a, b) => a.length === b.length && a.every((value, i) => value === b[i]),
    );
    const first = [1, 2, 3];
    const out = select(first);
    expect(select(first)).toBe(out);
    expect(runs).toBe(1);
    expect(select([0, 2, 3])).toBe(out);
    expect(select([2])).toEqual([2]);
  });
});

describe("combineStores", () => {
  it("computes again only when one of its stores changed", () => {
    const a = createValueStore(1);
    const b = createValueStore(10);
    let runs = 0;
    const sum = combineStores(a, b, (x, y) => {
      runs++;
      return { total: x + y };
    });
    const first = sum.getSnapshot();
    expect(sum.getSnapshot()).toBe(first);
    expect(runs).toBe(1);
    let calls = 0;
    const off = sum.subscribe(() => calls++);
    b.set(20);
    expect(calls).toBe(1);
    expect(sum.getSnapshot()).toEqual({ total: 21 });
    off();
    a.set(2);
    expect(calls).toBe(1);
  });
});
