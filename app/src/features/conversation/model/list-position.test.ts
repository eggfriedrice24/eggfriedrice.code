import { describe, expect, it } from "vitest";

import {
  createEndFollow,
  distanceToEnd,
  FOLLOW_DISTANCE,
  isNearEnd,
  type ContentMark,
} from "./list-position";

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

describe("distanceToEnd", () => {
  it("counts from the offset to the largest offset, with the inset under the content", () => {
    const contentSize = { height: 5000 };
    expect(
      distanceToEnd({ contentOffset: { y: 4200 }, contentSize, layoutMeasurement: SCREEN }),
    ).toBe(0);
    expect(
      distanceToEnd({
        contentOffset: { y: 4000 },
        contentSize,
        layoutMeasurement: SCREEN,
        contentInset: { bottom: 100 },
      }),
    ).toBe(300);
  });
});

/** A scroll of a 5000 pt content at `distance` from its end. */
function at(distance: number) {
  return {
    contentOffset: { y: 4200 - distance },
    contentSize: { height: 5000 },
    layoutMeasurement: SCREEN,
  };
}

const UI_CLOSED = new Map();
const UI_OPEN = new Map([["t:1", new Set(["expanded"])]]);

function mark(change: Partial<ContentMark> = {}): ContentMark {
  return { seq: 10, running: undefined, rows: 4, rowUi: UI_CLOSED, ...change };
}

describe("the rule of the end", () => {
  it("follows the first content and each new event while the reader is at the end", () => {
    const follow = createEndFollow();
    expect(follow.contentChanged(mark())).toBe(true);
    expect(follow.contentChanged(mark({ seq: 11 }))).toBe(true);
    expect(follow.contentChanged(mark({ seq: 11, rows: 5 }))).toBe(true);
    const turn = {};
    expect(follow.contentChanged(mark({ seq: 11, rows: 5, running: turn }))).toBe(true);
    // A second layout pass of the same content keeps the answer.
    expect(follow.contentChanged(mark({ seq: 11, rows: 5, running: turn }))).toBe(true);
  });

  it("never follows a row that the user opens or closes, in any of its layout passes", () => {
    const follow = createEndFollow();
    follow.contentChanged(mark());
    expect(follow.contentChanged(mark({ rowUi: UI_OPEN }))).toBe(false);
    expect(follow.contentChanged(mark({ rowUi: UI_OPEN }))).toBe(false);
    expect(follow.contentChanged(mark({ rowUi: UI_CLOSED }))).toBe(false);
    // New content after it follows again.
    expect(follow.contentChanged(mark({ seq: 11, rowUi: UI_CLOSED }))).toBe(true);
  });

  it("stops to follow when the user scrolls away, and follows again at the end", () => {
    const follow = createEndFollow();
    follow.contentChanged(mark());
    follow.dragStarted();
    follow.scrolled(at(FOLLOW_DISTANCE + 1));
    expect(follow.contentChanged(mark({ seq: 11 }))).toBe(false);
    follow.scrolled(at(FOLLOW_DISTANCE));
    expect(follow.contentChanged(mark({ seq: 12 }))).toBe(true);
    follow.dragEnded(at(2000));
    expect(follow.contentChanged(mark({ seq: 13 }))).toBe(false);
    follow.momentumStarted();
    follow.momentumEnded(at(0));
    expect(follow.contentChanged(mark({ seq: 14 }))).toBe(true);
  });

  it("does not count the scrolls of the list itself", () => {
    const follow = createEndFollow();
    follow.contentChanged(mark());
    // A scroll event with no finger on the list, such as during its own scroll to the end.
    follow.scrolled(at(3000));
    // iOS ends an animated scroll of the list with a momentum end and no start.
    follow.momentumEnded(at(3000));
    follow.dragEnded(at(3000));
    expect(follow.contentChanged(mark({ seq: 11 }))).toBe(true);
  });

  it("follows after a jump to the end, and not after a scroll to a row", () => {
    const follow = createEndFollow();
    follow.contentChanged(mark());
    follow.movedAway();
    expect(follow.contentChanged(mark({ seq: 11 }))).toBe(false);
    follow.jumped();
    expect(follow.contentChanged(mark({ seq: 12 }))).toBe(true);
  });
});
