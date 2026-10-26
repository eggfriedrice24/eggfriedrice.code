import { describe, expect, it } from "vitest";

import { between, clockTime, duration, elapsed, ELAPSED_FROM_MS } from "./durations";

describe("duration", () => {
  it("writes tenths under a second", () => {
    expect(duration(0)).toBe("0s");
    expect(duration(240)).toBe("0.2s");
    expect(duration(999)).toBe("0.9s");
  });

  it("writes whole seconds under a minute", () => {
    expect(duration(1000)).toBe("1s");
    expect(duration(9_400)).toBe("9s");
    expect(duration(38_000)).toBe("38s");
  });

  it("writes minutes and two-digit seconds under an hour", () => {
    expect(duration(62_000)).toBe("1m 02s");
    expect(duration(72_000)).toBe("1m 12s");
    expect(duration(188_000)).toBe("3m 08s");
  });

  it("writes hours and two-digit minutes from an hour", () => {
    expect(duration(3_720_000)).toBe("1h 02m");
  });

  it("never goes below zero", () => {
    expect(duration(-5)).toBe("0s");
  });
});

describe("between", () => {
  it("needs both ends", () => {
    expect(between(undefined, 5000)).toBeUndefined();
    expect(between(1000, undefined)).toBeUndefined();
    expect(between(1000, 23_000)).toBe("22s");
  });
});

describe("elapsed", () => {
  it("shows nothing under 2 s (design 3.20)", () => {
    expect(elapsed(10_000, 10_000 + ELAPSED_FROM_MS - 1)).toBeUndefined();
    expect(elapsed(10_000, 12_000)).toBe("2s");
  });

  it("counts whole seconds", () => {
    expect(elapsed(0, 12_900)).toBe("12s");
    expect(elapsed(0, 72_500)).toBe("1m 12s");
  });

  it("shows nothing without a start", () => {
    expect(elapsed(undefined, 50_000)).toBeUndefined();
  });
});

describe("clockTime", () => {
  it("writes the local hour and minute with two digits", () => {
    expect(clockTime(new Date(2026, 9, 26, 14, 32, 59).getTime())).toBe("14:32");
    expect(clockTime(new Date(2026, 9, 26, 9, 5).getTime())).toBe("09:05");
  });
});
