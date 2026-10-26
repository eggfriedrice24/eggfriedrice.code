import { describe, expect, it } from "vitest";

import { isNearEnd } from "./list-position";

const SCREEN = { height: 800 };

describe("isNearEnd", () => {
  it("is true at the end and within one screen of it", () => {
    const contentSize = { height: 5000 };
    expect(isNearEnd({ contentOffset: { y: 4200 }, contentSize, layoutMeasurement: SCREEN })).toBe(
      true,
    );
    expect(isNearEnd({ contentOffset: { y: 3400 }, contentSize, layoutMeasurement: SCREEN })).toBe(
      true,
    );
  });

  it("is false more than one screen above the end", () => {
    const contentSize = { height: 5000 };
    expect(isNearEnd({ contentOffset: { y: 3399 }, contentSize, layoutMeasurement: SCREEN })).toBe(
      false,
    );
    expect(isNearEnd({ contentOffset: { y: 0 }, contentSize, layoutMeasurement: SCREEN })).toBe(
      false,
    );
  });

  it("counts the inset of the keyboard as part of the end", () => {
    const scroll = {
      contentOffset: { y: 3400 },
      contentSize: { height: 5000 },
      layoutMeasurement: SCREEN,
      contentInset: { bottom: 300 },
    };
    expect(isNearEnd(scroll)).toBe(false);
  });

  it("is true for content shorter than the screen", () => {
    expect(
      isNearEnd({
        contentOffset: { y: 0 },
        contentSize: { height: 300 },
        layoutMeasurement: SCREEN,
      }),
    ).toBe(true);
  });
});
