import { describe, expect, it } from "vitest";

import { minuteOf } from "./use-now";

describe("the minute clock", () => {
  it("rounds a time down to the minute", () => {
    expect(minuteOf(0)).toBe(0);
    expect(minuteOf(59_999)).toBe(0);
    expect(minuteOf(60_000)).toBe(60_000);
    expect(minuteOf(125_000)).toBe(120_000);
  });
});
