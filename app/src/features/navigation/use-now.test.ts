import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TICK_MS, minuteOf, subscribeMinute } from "./use-now";

describe("the minute clock", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("rounds a time down to the minute", () => {
    expect(minuteOf(0)).toBe(0);
    expect(minuteOf(59_999)).toBe(0);
    expect(minuteOf(60_000)).toBe(60_000);
    expect(minuteOf(125_000)).toBe(120_000);
  });

  it("shares one timer for every reader and stops it after the last one", () => {
    const first = vi.fn<() => void>();
    const second = vi.fn<() => void>();
    const stopFirst = subscribeMinute(first);
    const stopSecond = subscribeMinute(second);
    expect(vi.getTimerCount()).toBe(1);
    vi.advanceTimersByTime(TICK_MS);
    expect([first.mock.calls.length, second.mock.calls.length]).toEqual([1, 1]);
    stopFirst();
    vi.advanceTimersByTime(TICK_MS);
    expect([first.mock.calls.length, second.mock.calls.length]).toEqual([1, 2]);
    expect(vi.getTimerCount()).toBe(1);
    stopSecond();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("starts again for a new reader", () => {
    const reader = vi.fn<() => void>();
    subscribeMinute(() => undefined)();
    expect(vi.getTimerCount()).toBe(0);
    const stop = subscribeMinute(reader);
    vi.advanceTimersByTime(4 * TICK_MS);
    expect(reader).toHaveBeenCalledTimes(4);
    stop();
  });
});
