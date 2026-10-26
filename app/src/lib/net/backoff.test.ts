import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import {
  backoffCeilingMs,
  backoffConnected,
  backoffDelayMs,
  backoffFailed,
  INITIAL_BACKOFF,
} from "./backoff";

describe("backoff", () => {
  it("doubles the ceiling from 2 s up to 300 s", () => {
    expect([0, 1, 2, 3, 7, 8, 9, 100].map((attempt) => backoffCeilingMs(attempt))).toEqual([
      2_000, 4_000, 8_000, 16_000, 256_000, 300_000, 300_000, 300_000,
    ]);
  });

  it("waits from half the ceiling to the ceiling", () => {
    expect(backoffDelayMs(0, 0)).toBe(1_000);
    expect(backoffDelayMs(0, 0.5)).toBe(1_500);
    expect(backoffDelayMs(3, 0.999_999)).toBe(15_999);
    fc.assert(
      fc.property(
        fc.nat({ max: 1_000 }),
        fc.double({ min: 0, max: 1, noNaN: true }),
        (attempt, random) => {
          const ceiling = backoffCeilingMs(attempt);
          const delay = backoffDelayMs(attempt, random);
          expect(delay).toBeGreaterThanOrEqual(ceiling / 2);
          expect(delay).toBeLessThanOrEqual(ceiling);
        },
      ),
    );
  });

  it("climbs on each failure and resets after 30 s connected", () => {
    let state = INITIAL_BACKOFF;
    const delays: number[] = [];
    for (let i = 0; i < 3; i++) {
      const failed = backoffFailed(state, i, 0);
      delays.push(failed.delayMs);
      state = failed.state;
    }
    expect(delays).toEqual([1_000, 2_000, 4_000]);

    const short = backoffFailed(backoffConnected(state, 1_000), 30_999, 0);
    expect(short.delayMs).toBe(8_000);

    const stable = backoffFailed(backoffConnected(short.state, 40_000), 70_000, 0);
    expect(stable.delayMs).toBe(1_000);
    expect(stable.state.attempt).toBe(1);
  });
});
