import * as fc from "fast-check";
import { describe, expect, it } from "vitest";

import {
  createPacer,
  DEFAULT_PACER_CONFIG,
  pacerArrive,
  pacerFrame,
  pacerJump,
  type Pacer,
  wordBoundaryAtOrBefore,
} from "./pacer";
import { isCodePointBoundary } from "./utf8";

const LAG = DEFAULT_PACER_CONFIG.maxLagMs;

describe("pacerFrame", () => {
  it("reveals a burst over the lag, by word", () => {
    const text = "one two three four";
    let pacer = pacerArrive(createPacer(0, 0), text.length, 0);
    pacer = pacerFrame(pacer, text, 0);
    expect(pacer.shown).toBe(0);
    pacer = pacerFrame(pacer, text, LAG / 2);
    expect(pacer.shown).toBe("one two ".length);
    pacer = pacerFrame(pacer, text, LAG);
    expect(pacer.shown).toBe(text.length);
    expect(pacer.arrivals).toEqual([]);
  });

  it("moves at the speed the text arrives", () => {
    const text = "a b c d e f g h ";
    let pacer = createPacer(0, 0);
    pacer = pacerArrive(pacer, 8, 0);
    pacer = pacerArrive(pacer, 16, 100);
    pacer = pacerFrame(pacer, text, LAG);
    expect(pacer.shown).toBe(8);
    pacer = pacerFrame(pacer, text, LAG + 50);
    expect(pacer.shown).toBe(12);
    pacer = pacerFrame(pacer, text, LAG + 100);
    expect(pacer.shown).toBe(16);
  });

  it("shows everything at once with Reduce Motion", () => {
    const text = "all of it";
    const pacer = pacerFrame(pacerArrive(createPacer(), text.length, 0), text, 1, {
      reduceMotion: true,
    });
    expect(pacer.shown).toBe(text.length);
  });

  it("jumps on catch-up and on pacerJump", () => {
    const big = "x ".repeat(DEFAULT_PACER_CONFIG.catchUpUnits);
    const caught = pacerArrive(createPacer(), big.length, 0);
    expect(caught.shown).toBe(big.length);
    expect(pacerJump(createPacer(), 5, 0).shown).toBe(5);
  });

  it("returns the same pacer when nothing changed", () => {
    const pacer = createPacer(3, 0);
    expect(pacerFrame(pacer, "abc", 10)).toBe(pacer);
  });
});

describe("wordBoundaryAtOrBefore", () => {
  it("stops after whitespace and after CJK characters", () => {
    expect(wordBoundaryAtOrBefore("hello world", 8)).toBe(6);
    expect(wordBoundaryAtOrBefore("hello", 3)).toBe(0);
    expect(wordBoundaryAtOrBefore("hello", 3, 2)).toBe(2);
    expect(wordBoundaryAtOrBefore("中文字", 2)).toBe(2);
    expect(wordBoundaryAtOrBefore("abc", 9)).toBe(3);
  });
});

describe("properties", () => {
  const arrivals = fc.array(
    fc.record({ units: fc.integer({ min: 1, max: 40 }), gap: fc.integer({ min: 0, max: 200 }) }),
    { maxLength: 30 },
  );
  const frames = fc.array(fc.integer({ min: 0, max: 60 }), { maxLength: 80 });

  it("never goes back, never passes the text, and is never later than the lag", () => {
    fc.assert(
      fc.property(arrivals, frames, fc.string({ unit: "binary" }), (plan, steps, filler) => {
        // The text grows by each arrival; `filler` gives it real characters.
        const unit = filler.length > 0 ? filler : "w ";
        let text = "";
        const due: { end: number; at: number }[] = [];
        let pacer: Pacer = createPacer(0, 0);
        let now = 0;
        let next = 0;
        let arrivalAt = 0;
        for (const step of steps) {
          now += step;
          while (next < plan.length) {
            const arrival = plan[next];
            if (arrival === undefined || arrivalAt + arrival.gap > now) {
              break;
            }
            arrivalAt += arrival.gap;
            text += unit.repeat(Math.ceil(arrival.units / unit.length));
            pacer = pacerArrive(pacer, text.length, arrivalAt);
            due.push({ end: text.length, at: arrivalAt });
            next++;
          }
          const before = pacer.shown;
          pacer = pacerFrame(pacer, text, now);
          expect(pacer.shown).toBeGreaterThanOrEqual(before);
          expect(pacer.shown).toBeLessThanOrEqual(text.length);
          expect(isCodePointBoundary(text, pacer.shown)).toBe(true);
          const mustShow = Math.max(
            0,
            ...due.filter((arrival) => arrival.at + LAG <= now).map((arrival) => arrival.end),
          );
          expect(pacer.shown).toBeGreaterThanOrEqual(mustShow);
        }
      }),
    );
  });
});
