import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createTicker } from "./ticker";

const MS = 1000;

describe("the shared ticker", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("shares one timer for every reader and stops it after the last one", () => {
    const ticker = createTicker(MS);
    const first = vi.fn<() => void>();
    const second = vi.fn<() => void>();
    const stopFirst = ticker.subscribe(first);
    const stopSecond = ticker.subscribe(second);
    expect(vi.getTimerCount()).toBe(1);
    expect(ticker.running()).toBe(true);
    vi.advanceTimersByTime(MS);
    expect([first.mock.calls.length, second.mock.calls.length]).toEqual([1, 1]);
    stopFirst();
    vi.advanceTimersByTime(MS);
    expect([first.mock.calls.length, second.mock.calls.length]).toEqual([1, 2]);
    expect(vi.getTimerCount()).toBe(1);
    stopSecond();
    expect(vi.getTimerCount()).toBe(0);
    expect(ticker.running()).toBe(false);
  });

  it("starts again for a new reader", () => {
    const ticker = createTicker(MS);
    const reader = vi.fn<() => void>();
    ticker.subscribe(() => undefined)();
    expect(vi.getTimerCount()).toBe(0);
    const stop = ticker.subscribe(reader);
    vi.advanceTimersByTime(4 * MS);
    expect(reader).toHaveBeenCalledTimes(4);
    stop();
  });
});
