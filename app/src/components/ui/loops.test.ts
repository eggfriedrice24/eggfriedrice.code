import { describe, expect, it } from "vitest";

import { BRAILLE } from "@/theme/metrics";

import {
  BLINK_LOOP,
  BRAILLE_DOTS,
  BRAILLE_LOOP,
  BRAILLE_STILL,
  blinkTable,
  brailleDots,
  brailleOpacity,
  brailleTable,
  stepTable,
  tableValue,
} from "./loops";

/** The opacities of the 8 dots on `step`. */
function frame(step: number): number[] {
  return BRAILLE_DOTS.map((_, dot) => brailleOpacity(step, dot));
}

describe("the braille loop", () => {
  it("runs 8 steps in 0.8 s, so one step is 100 ms", () => {
    expect(BRAILLE_LOOP).toEqual({ cycleMs: 800, steps: 8 });
    expect(BRAILLE_DOTS).toHaveLength(8);
  });

  it("lights dot 0 first with the trail on dots 7 and 6", () => {
    expect(frame(0)).toEqual([1, 0.16, 0.16, 0.16, 0.16, 0.16, 0.4, 0.7]);
  });

  it("moves the lit dot one place clockwise on each step", () => {
    expect(frame(1)).toEqual([0.7, 1, 0.16, 0.16, 0.16, 0.16, 0.16, 0.4]);
    for (let step = 0; step < BRAILLE_LOOP.steps; step++) {
      expect(frame(step).indexOf(1)).toBe(step);
    }
  });

  it("has exactly one lit dot and a trail of two on every step", () => {
    for (let step = 0; step < BRAILLE_LOOP.steps; step++) {
      const sorted = [...frame(step)].sort((a, b) => b - a);
      expect(sorted).toEqual([1, 0.7, 0.4, 0.16, 0.16, 0.16, 0.16, 0.16]);
    }
  });

  it("walks clockwise: each dot is next to the one before it", () => {
    BRAILLE_DOTS.forEach((dot, index) => {
      const next = BRAILLE_DOTS[(index + 1) % BRAILLE_DOTS.length];
      const distance = Math.abs((next?.x ?? 0) - dot.x) + Math.abs((next?.y ?? 0) - dot.y);
      expect(distance).toBe(3);
    });
  });

  it("has the still frame of the design: dots 0 and 1 lit, dot 7 at half", () => {
    expect(BRAILLE_STILL).toEqual([1, 1, 0.16, 0.16, 0.16, 0.16, 0.16, 0.5]);
  });
});

describe("the dots of a braille cell", () => {
  it("fit inside each size of the metrics", () => {
    for (const size of Object.values(BRAILLE)) {
      for (const dot of brailleDots(size.width, size.height)) {
        expect(dot.left).toBeGreaterThanOrEqual(0);
        expect(dot.top).toBeGreaterThanOrEqual(0);
        expect(dot.left + dot.size).toBeLessThanOrEqual(size.width);
        expect(dot.top + dot.size).toBeLessThanOrEqual(size.height);
      }
    }
  });

  it("scales the 6 x 12 box of the design", () => {
    const [first, second] = brailleDots(6, 12);
    expect(first).toEqual({ dot: 0, left: 1.5 - 1.1, top: 1.5 - 1.1, size: 2.2 });
    expect(second?.left).toBeCloseTo(4.5 - 1.1);
  });
});

describe("the step tables", () => {
  it("hold one value over each whole step", () => {
    const table = stepTable([1, 0.5]);
    expect(tableValue(table, 0)).toBe(1);
    expect(tableValue(table, 0.99)).toBe(1);
    expect(tableValue(table, 1)).toBe(0.5);
    expect(tableValue(table, 1.99)).toBe(0.5);
  });

  it("give each dot its opacity on each step of the clock", () => {
    for (let dot = 0; dot < 8; dot++) {
      const table = brailleTable(dot);
      for (let step = 0; step < BRAILLE_LOOP.steps; step++) {
        expect(tableValue(table, step + 0.5)).toBe(brailleOpacity(step, dot));
      }
    }
  });

  it("blink on for the first half of 1.06 s and low for the second half", () => {
    expect(BLINK_LOOP).toEqual({ cycleMs: 1060, steps: 2 });
    const table = blinkTable(0.25);
    expect(tableValue(table, 0.2)).toBe(1);
    expect(tableValue(table, 1.2)).toBe(0.25);
  });

  it("are tables that RN Animated takes: rising inputs of the same length as the outputs", () => {
    for (const table of [blinkTable(0), brailleTable(3)]) {
      expect(table.inputRange).toHaveLength(table.outputRange.length);
      const rising = table.inputRange.every(
        (value, index) => index === 0 || value > (table.inputRange[index - 1] ?? 0),
      );
      expect(rising).toBe(true);
    }
  });
});
