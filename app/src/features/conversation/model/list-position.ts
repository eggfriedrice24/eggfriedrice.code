// Where the reader is in the transcript list. The list is not inverted, so the newest
// row is at the end: the largest offset.

/** The part of a scroll event of React Native that the list reads. */
export interface ListScroll {
  readonly contentOffset: { readonly y: number };
  readonly contentSize: { readonly height: number };
  readonly layoutMeasurement: { readonly height: number };
  /** The inset under the content, such as the keyboard. iOS gives it. */
  readonly contentInset?: { readonly bottom: number };
}

/** True when the reader is within one screen of the end. */
export function isNearEnd(scroll: ListScroll): boolean {
  return distanceToEnd(scroll) <= scroll.layoutMeasurement.height;
}

/**
 * The distance to the end, in pt, from where the reader is now. The end is the largest
 * offset: the content and the inset under it, less the height of the view.
 */
export function distanceToEnd(scroll: ListScroll): number {
  const { contentOffset, contentSize, layoutMeasurement } = scroll;
  const inset = scroll.contentInset?.bottom ?? 0;
  const end = Math.max(0, contentSize.height + inset - layoutMeasurement.height);
  return end - contentOffset.y;
}

/**
 * New content scrolls the list with it while the reader is this close to the end, in
 * pt. NOTE: FlashList reads its `autoscrollToBottomThreshold` in screens, not in pt, so
 * the list does not use it: 200 there followed the end from anywhere.
 */
export const FOLLOW_DISTANCE = 200;

/** What the content holds when its size changes: the list compares two of them. */
export interface ContentMark {
  /** The sequence number of the newest event of the stream. */
  readonly seq: number;
  /** The running turn: the stream makes a new object for each draft. */
  readonly running: unknown;
  /** The number of rows. */
  readonly rows: number;
  /** The state of the rows that the user opens and closes (model/row-ui-state.ts). */
  readonly rowUi: unknown;
}

/** True when `a` and `b` hold the same events, the same running turn and as many rows. */
function sameContent(a: ContentMark, b: ContentMark): boolean {
  return a.seq === b.seq && a.running === b.running && a.rows === b.rows;
}

/**
 * The rule of the list for the end (design 3.12). The list starts at the end. While the
 * reader stays within FOLLOW_DISTANCE of it, new content (an event, a draft of the
 * running turn, a new row) scrolls the list to the end. A row that the user opens or
 * closes is not new content: the list does not move for it, so the row stays where it
 * is on the screen. Only a scroll of the user moves the reader away from the end or
 * back; the scrolls of the list itself do not count.
 */
export interface EndFollow {
  /** A scroll event. It counts only while the user drags, or the list coasts after it. */
  scrolled(scroll: ListScroll): void;
  /** The user puts a finger on the list. */
  dragStarted(): void;
  /** The user lifts the finger, at `scroll`. */
  dragEnded(scroll: ListScroll): void;
  /** The list coasts after a drag. */
  momentumStarted(): void;
  /** The list stops at `scroll` after it coasted. */
  momentumEnded(scroll: ListScroll): void;
  /** The list scrolled to the end on request, such as "Jump to latest": it follows. */
  jumped(): void;
  /** The list scrolled to a row on request, such as an approval: it stops to follow. */
  movedAway(): void;
  /**
   * The size of the content changed, and the content is `mark` now. True when the list
   * must scroll to the end. A change with the same content as the last one, such as a
   * second layout pass, gets the answer of the last one.
   */
  contentChanged(mark: ContentMark): boolean;
}

/** A new rule for one list. It follows the end at first. */
export function createEndFollow(): EndFollow {
  let following = true;
  let dragging = false;
  let coasting = false;
  let last: ContentMark | undefined;
  // True when the last change of the content was new content, not a row of the user.
  let fresh = true;
  const read = (scroll: ListScroll): void => {
    following = distanceToEnd(scroll) <= FOLLOW_DISTANCE;
  };
  return {
    scrolled(scroll) {
      if (dragging || coasting) {
        read(scroll);
      }
    },
    dragStarted() {
      dragging = true;
    },
    dragEnded(scroll) {
      if (dragging) {
        dragging = false;
        read(scroll);
      }
    },
    momentumStarted() {
      coasting = true;
    },
    momentumEnded(scroll) {
      // iOS also ends an animated scroll of the list itself with this event, with no
      // start before it. That one does not count.
      if (coasting) {
        coasting = false;
        read(scroll);
      }
    },
    jumped() {
      following = true;
    },
    movedAway() {
      following = false;
    },
    contentChanged(mark) {
      if (last === undefined || !sameContent(mark, last)) {
        fresh = true;
      } else if (mark.rowUi !== last.rowUi) {
        fresh = false;
      }
      last = mark;
      return fresh && following;
    },
  };
}
