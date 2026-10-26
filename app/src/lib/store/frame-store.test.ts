import { describe, expect, it } from "vitest";

import { manualFrames } from "@/lib/stream/testing";
import { createFrameStore, memoSelector } from "./frame-store";

describe("createFrameStore", () => {
  it("writes the newest value to the store once per frame", () => {
    const frames = manualFrames();
    const frame = createFrameStore(0, frames.schedule);
    const seen: number[] = [];
    frame.store.subscribe((value) => seen.push(value));
    frame.set(1);
    frame.set(2);
    frame.set(3);
    expect(frame.latest()).toBe(3);
    expect(frame.store.get()).toBe(0);
    expect(frames.pending()).toBe(1);
    frames.frame();
    expect(frame.store.get()).toBe(3);
    expect(seen).toEqual([3]);
    frames.frame();
    expect(seen).toEqual([3]);
  });

  it("asks for no frame for the same value, and tells nobody when a frame changes nothing", () => {
    const frames = manualFrames();
    const value = { n: 1 };
    const frame = createFrameStore(value, frames.schedule);
    let calls = 0;
    frame.store.subscribe(() => calls++);
    frame.set(value);
    expect(frames.pending()).toBe(0);
    frame.set({ n: 2 });
    frame.set(value);
    frames.frame();
    expect(calls).toBe(0);
    expect(frame.store.get()).toBe(value);
  });

  it("flushes at once and cancels the frame", () => {
    const frames = manualFrames();
    const frame = createFrameStore("a", frames.schedule);
    frame.set("b");
    frame.flush();
    expect(frame.store.get()).toBe("b");
    expect(frames.pending()).toBe(0);
  });

  it("writes each value at once with a scheduler that runs the callback at once", () => {
    const frame = createFrameStore(0, (callback) => {
      callback();
      return () => undefined;
    });
    frame.set(1);
    expect(frame.store.get()).toBe(1);
    frame.set(2);
    expect(frame.store.get()).toBe(2);
  });

  it("asks for no frame after dispose, and keeps the newest value", () => {
    const frames = manualFrames();
    const frame = createFrameStore(1, frames.schedule);
    frame.set(2);
    frame.dispose();
    expect(frames.pending()).toBe(0);
    frame.set(3);
    expect(frames.pending()).toBe(0);
    expect(frame.latest()).toBe(3);
    expect(frame.store.get()).toBe(1);
  });
});

describe("memoSelector", () => {
  it("keeps its result for the same input and for an equal result", () => {
    let runs = 0;
    const select = memoSelector(
      (value: { readonly items: readonly number[] }) => {
        runs++;
        return value.items.filter((n) => n > 1);
      },
      (a, b) => a.length === b.length && a.every((n, i) => n === b[i]),
    );
    const first = { items: [1, 2, 3] };
    const result = select(first);
    expect(select(first)).toBe(result);
    expect(runs).toBe(1);
    expect(select({ items: [2, 3] })).toBe(result);
    expect(select({ items: [3] })).toEqual([3]);
  });
});
